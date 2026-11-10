// SPDX-License-Identifier: CC0-1.0

//! The Amplify UI Builder tagging surface over the wire: `ListTagsForResource`,
//! `TagResource` and `UntagResource`, all bound to `/tags/{resourceArn}`.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one ARN and
//! one tag:
//!
//! 1. ONE TAG SET PER RESOURCE. `ListTagsForResource` reports exactly the tags stored on
//!    the resource its `resourceArn` names, whichever of this service's resource kinds
//!    that is, and the resource's OWN reader reports the same set: a create that supplied
//!    tags is read back by both, and a resource created without any answers the empty map
//!    (`tags` is `@required` on `ListTagsForResourceResponse`).
//! 2. ROUND TRIP. `TagResource` is additive — a key the resource carries takes the new
//!    value, every other key it carries is kept — and `UntagResource` removes exactly the
//!    named keys and is idempotent. What either writes is what `ListTagsForResource` and
//!    the resource's own reader read back, because there is one stored tag set, not two.
//! 3. ABSENCE. An ARN that names no resource this store holds — an unknown id, another app
//!    environment, a deleted resource, a collection this service names no resource kind
//!    with, another service's ARN, or a string that is not an ARN at all — is refused on
//!    all three operations with HTTP 404 and this service's one not-found literal, and
//!    changes nothing.
//! 4. The partition, region and account elements of the ARN do not decide which resource
//!    it names: this store keys every resource by (appId, environmentName, id) alone and
//!    the emulator serves a single account, so the same resource is addressed through any
//!    of them.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment a case addresses unless it is about addressing another
/// one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The region and account elements the ARNs built here carry: the region the requests are
/// signed for, and the one account id a generated emulator presents
/// (`engine::emulator_account_id`).
const REGION: &str = "eu-west-1";
const ACCOUNT: &str = "000000000000";

/// The collection element of each resource kind's documented ARN form
/// (`arn:aws:amplifyuibuilder:<region>:<account>:app/<appId>/environment/<environmentName>/<collection>/<id>`),
/// which is also the collection element of that kind's own REST URIs
/// (`/app/{appId}/environment/{environmentName}/components/{id}`, see this crate's
/// `OPERATIONS.md`) — one per resource kind of this service whose record holds a tag set
/// (`Component$tags`, `Form$tags`, `Theme$tags`, `CodegenJob$tags`).
const COLLECTIONS: [&str; 4] = ["components", "forms", "themes", "codegen-jobs"];

/// The message of a not-found refusal on a reporting call — this service's one literal for
/// an identifier that names no resource, asserted in `resource_error_messages.rs` too.
const NOT_FOUND_MESSAGE: &str = "The item specified was not found";

/// The published bounds of a tag key and a tag value:
/// `smithy_query(query="shape", shape="Tags")` gives `TagKey` `@length(min: 1, max: 128)`
/// and `TagValue` `@length(min: 1, max: 256)`.
const TAG_KEY_MAX_LENGTH: usize = 128;
const TAG_VALUE_MAX_LENGTH: usize = 256;

/// A one-entry tag set, for a case whose key is computed rather than written out.
fn one_entry(key: &str, value: &str) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), json!(value));
    Value::Object(map)
}

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
    /// The resource id the answer carries, or the empty string. Every create and read of
    /// this service binds its resource to `@httpPayload`, so the body IS the resource.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The `message` the answer carries, in either spelling a JSON error body uses.
    fn message(&self) -> &str {
        ["message", "Message"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }

    /// The tag set the answer reports: the `tags` member as the wire carries it. Every
    /// tagged record of this service holds its tag set as a map, so a reader that reports
    /// the resource reports the member — `Value::Null` here is a missing member, which is
    /// a disagreement with `ListTagsForResource` rather than the empty set.
    fn tags(&self) -> Value {
        self.body.get("tags").cloned().unwrap_or(Value::Null)
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Percent-encode one value the way an SDK encodes a non-greedy `@httpLabel` or a query
/// value: everything outside the unreserved set is escaped, so a whole ARN is ONE path
/// segment.
fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// An ARN of this service naming one resource, with every element spelled out.
fn arn_with(
    partition: &str,
    region: &str,
    account: &str,
    collection: &str,
    app_id: &str,
    environment: &str,
    id: &str,
) -> String {
    format!(
        "arn:{partition}:amplifyuibuilder:{region}:{account}:app/{app_id}/environment/\
         {environment}/{collection}/{id}"
    )
}

/// The ARN of one resource as the emulator's own account and region name it.
fn arn(collection: &str, app_id: &str, environment: &str, id: &str) -> String {
    arn_with(
        "aws",
        REGION,
        ACCOUNT,
        collection,
        app_id,
        environment,
        id,
    )
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", encoded(resource_arn)), None).await
}

async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", encoded(resource_arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[String]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encoded(key)))
        .collect::<Vec<String>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", encoded(resource_arn)),
        None,
    )
    .await
}

/// The collection URI of one resource kind in one app environment.
fn collection_uri(collection: &str, app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/{collection}")
}

/// The create payload of one resource kind, carrying only the members that kind's
/// `Create…Data` requires, plus `tags` when the case supplies them.
fn create_payload(collection: &str, name: &str, tags: Option<&Value>) -> Value {
    let mut data = match collection {
        "components" => json!({
            "name": name,
            "componentType": "View",
            "properties": {},
            "variants": [],
            "overrides": {},
            "bindingProperties": {},
        }),
        "forms" => json!({
            "name": name,
            "formActionType": "create",
            "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
            "fields": {},
            "style": {},
            "sectionalElements": {},
            "schemaVersion": "1.0",
        }),
        "themes" => json!({"name": name, "values": []}),
        // A codegen job carries no name; `StartCodegenJobData` requires its render config.
        "codegen-jobs" => json!({"renderConfig": {"react": {}}}),
        other => panic!("no create payload is written for {other:?}"),
    };
    if let Some(tags) = tags {
        data["tags"] = tags.clone();
    }
    data
}

async fn create(
    router: &MultiServiceRouter,
    collection: &str,
    app_id: &str,
    environment: &str,
    name: &str,
    tags: Option<&Value>,
) -> Answer {
    call(
        router,
        "POST",
        &collection_uri(collection, app_id, environment),
        Some(create_payload(collection, name, tags)),
    )
    .await
}

async fn read_resource(
    router: &MultiServiceRouter,
    collection: &str,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("{}/{id}", collection_uri(collection, app_id, environment)),
        None,
    )
    .await
}

/// Both readers of one resource's tag set report `expected`: the tagging surface and the
/// resource's own `Get…`. One stored set, so the two can never disagree.
async fn assert_tags(
    case: &str,
    router: &MultiServiceRouter,
    collection: &str,
    id: &str,
    expected: &Value,
) {
    let listed = list_tags(router, &arn(collection, APP_ID, ENVIRONMENT, id)).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListTagsForResource must answer HTTP 200, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        &listed.tags(),
        expected,
        "case {case:?}: ListTagsForResource must report {expected:?}, got {:?}",
        listed.tags()
    );

    let read = read_resource(router, collection, APP_ID, ENVIRONMENT, id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the resource must still read back, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        &read.tags(),
        expected,
        "case {case:?}: the resource's own reader must report {expected:?}, got {:?}",
        read.tags()
    );
}

/// The tag sets a create supplies, differing along what the rule generalises over: how
/// many pairs, the form of a key and of a value, each published length bound, and whether
/// the create names `tags` at all.
fn tag_set_rows() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no tags member at all", None),
        ("an empty tag set", Some(json!({}))),
        ("one pair", Some(json!({"env": "test"}))),
        (
            "several pairs",
            Some(json!({"env": "test", "owner": "studio", "tier": "free"})),
        ),
        (
            "a key and a value of one character, the shortest the model admits",
            Some(json!({"k": "v"})),
        ),
        (
            "a key and a value at the longest length the model admits",
            Some(one_entry(
                &"k".repeat(TAG_KEY_MAX_LENGTH),
                &"v".repeat(TAG_VALUE_MAX_LENGTH),
            )),
        ),
        (
            "keys carrying the punctuation the key pattern admits",
            Some(json!({
                "cost-center": "17",
                "path/to:thing": "yes",
                "team.name_v2": "studio",
            })),
        ),
        (
            "values carrying spaces, punctuation and non-ASCII text",
            Some(json!({"note": "Grüße, zum Test — 100%"})),
        ),
    ]
}

/// The tag set a create persisted is what the tagging surface reports, for every resource
/// kind and every form of tag set.
#[tokio::test]
async fn the_tagging_surface_reports_the_tag_set_a_create_persisted() {
    let router = emulator();

    for collection in COLLECTIONS {
        for (index, (case, tags)) in tag_set_rows().into_iter().enumerate() {
            let case = format!("{case} on a {collection} resource");
            let created = create(
                &router,
                collection,
                APP_ID,
                ENVIRONMENT,
                &format!("Tagged{index}"),
                tags.as_ref(),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the create must succeed, got {} with {:?}",
                created.status, created.body
            );
            let expected = tags.unwrap_or_else(|| json!({}));
            assert_tags(&case, &router, collection, &created.id(), &expected).await;
        }
    }
}

/// `TagResource` merges, `UntagResource` removes exactly the named keys, both are
/// idempotent, and each write is read back by the tagging surface AND by the resource's
/// own reader.
#[tokio::test]
async fn a_tag_write_merges_and_a_removal_removes_exactly_the_named_keys() {
    let router = emulator();

    for collection in COLLECTIONS {
        let created = create(
            &router,
            collection,
            APP_ID,
            ENVIRONMENT,
            "Roundtrip",
            Some(&json!({"env": "test", "owner": "studio"})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "the {collection} create must succeed, got {} with {:?}",
            created.status, created.body
        );
        let id = created.id();
        let resource_arn = arn(collection, APP_ID, ENVIRONMENT, &id);
        let mut expected: Map<String, Value> = json!({"env": "test", "owner": "studio"})
            .as_object()
            .cloned()
            .expect("the base tag set is an object");

        // The writes differ along what the rule generalises over: a key the resource does
        // not carry, a key it does (whose value is replaced), several at once, and a pair
        // it already carries (which changes nothing).
        let writes: Vec<(&str, Value)> = vec![
            ("a key the resource does not carry", json!({"team": "ui"})),
            ("a key it carries takes the new value", json!({"env": "prod"})),
            (
                "several keys at once",
                json!({"cost-center": "17", "tier": "free"}),
            ),
            ("a pair it already carries", json!({"team": "ui"})),
        ];
        for (case, written) in writes {
            let case = format!("{case} on a {collection} resource");
            let reply = tag(&router, &resource_arn, &written).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: TagResource must answer HTTP 200, got {} with {:?}",
                reply.status, reply.body
            );
            for (key, value) in written.as_object().expect("the written tags are an object") {
                expected.insert(key.clone(), value.clone());
            }
            assert_tags(&case, &router, collection, &id, &Value::Object(expected.clone())).await;
        }

        // The removals differ the same way: one key, several, a key the resource does not
        // carry, and every remaining key — each one run TWICE, since removing a key that
        // is already gone is nothing to remove.
        let removals: Vec<(&str, Vec<String>)> = vec![
            ("one key", vec!["owner".to_string()]),
            (
                "several keys",
                vec!["cost-center".to_string(), "tier".to_string()],
            ),
            (
                "a key the resource does not carry",
                vec!["nothing-carries-this".to_string()],
            ),
            (
                "every remaining key",
                vec!["env".to_string(), "team".to_string()],
            ),
        ];
        for (case, keys) in removals {
            let case = format!("{case} on a {collection} resource");
            for key in &keys {
                expected.remove(key);
            }
            for round in 1..=2 {
                let reply = untag(&router, &resource_arn, &keys).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} (round {round}): UntagResource must answer HTTP 200, got {} \
                     with {:?}",
                    reply.status, reply.body
                );
                assert_tags(
                    &format!("{case} (round {round})"),
                    &router,
                    collection,
                    &id,
                    &Value::Object(expected.clone()),
                )
                .await;
            }
        }

        assert!(
            expected.is_empty(),
            "case {collection:?}: the removals must leave no tag, got {expected:?}"
        );
    }
}

/// An ARN that names no resource this store holds is absent on every tagging operation:
/// HTTP 404 with this service's one not-found literal, and nothing written.
#[tokio::test]
async fn an_arn_that_names_no_resource_is_absent_on_every_tagging_operation() {
    let router = emulator();

    // A live resource of each kind, so an absent ARN is told apart from an empty store —
    // and so a refused call can be shown to have changed nothing.
    let mut live: Vec<(&str, String)> = Vec::new();
    for collection in COLLECTIONS {
        let created = create(
            &router,
            collection,
            APP_ID,
            ENVIRONMENT,
            "Live",
            Some(&json!({"env": "test"})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "the {collection} create must succeed, got {} with {:?}",
            created.status, created.body
        );
        live.push((collection, created.id()));
    }

    // A component created and then deleted: its ARN named a resource once and names none
    // now.
    let deleted = create(
        &router,
        "components",
        APP_ID,
        ENVIRONMENT,
        "Ephemeral",
        Some(&json!({"env": "test"})),
    )
    .await;
    let deleted_id = deleted.id();
    let dropped = call(
        &router,
        "DELETE",
        &format!(
            "{}/{deleted_id}",
            collection_uri("components", APP_ID, ENVIRONMENT)
        ),
        None,
    )
    .await;
    assert_eq!(
        dropped.status, 200,
        "deleting the ephemeral component must succeed, got {} with {:?}",
        dropped.status, dropped.body
    );

    let live_component = live
        .iter()
        .find(|(collection, _)| *collection == "components")
        .map(|(_, id)| id.clone())
        .expect("a live component was created");

    // The rows differ along WHY the ARN names nothing: the id, the app environment, the
    // resource's lifetime, the collection, the service, and the form of the string itself.
    let absent: Vec<(&str, String)> = vec![
        (
            "an id no resource of this app environment holds",
            arn(
                "components",
                APP_ID,
                ENVIRONMENT,
                "11111111-1111-4111-8111-111111111111",
            ),
        ),
        (
            "a live id under another app",
            arn("components", "d9999zzzz", ENVIRONMENT, &live_component),
        ),
        (
            "a live id under another environment",
            arn("components", APP_ID, "production", &live_component),
        ),
        (
            "a deleted resource",
            arn("components", APP_ID, ENVIRONMENT, &deleted_id),
        ),
        (
            "an id that names no resource of the ARN's collection",
            arn("codegen-jobs", APP_ID, ENVIRONMENT, &live_component),
        ),
        (
            "a collection this service names no resource kind with",
            arn("widgets", APP_ID, ENVIRONMENT, &live_component),
        ),
        (
            "another service's ARN",
            format!("arn:aws:amplify:{REGION}:{ACCOUNT}:apps/{APP_ID}"),
        ),
        (
            "an ARN whose resource path is not an app environment's",
            format!("arn:aws:amplifyuibuilder:{REGION}:{ACCOUNT}:component/{live_component}"),
        ),
        (
            "an ARN missing the resource element",
            format!("arn:aws:amplifyuibuilder:{REGION}:{ACCOUNT}"),
        ),
        (
            "an ARN with an empty id",
            arn("components", APP_ID, ENVIRONMENT, ""),
        ),
        ("a string that is not an ARN at all", "not-an-arn".to_string()),
    ];

    for (case, resource_arn) in &absent {
        for (operation, answer) in [
            ("ListTagsForResource", list_tags(&router, resource_arn).await),
            (
                "TagResource",
                tag(&router, resource_arn, &json!({"env": "prod"})).await,
            ),
            (
                "UntagResource",
                untag(&router, resource_arn, &["env".to_string()]).await,
            ),
        ] {
            let case = format!("{case} on {operation}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: an ARN naming no resource must answer HTTP 404, got {} with \
                 {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                NOT_FOUND_MESSAGE,
                "case {case:?}: expected the message {NOT_FOUND_MESSAGE:?}, got {:?}",
                answer.message()
            );
        }
    }

    // None of those refusals touched a stored tag set: every live resource still carries
    // exactly what its create persisted.
    for (collection, id) in &live {
        assert_tags(
            &format!("after the refusals, the live {collection} resource"),
            &router,
            collection,
            id,
            &json!({"env": "test"}),
        )
        .await;
    }
}

/// The partition, region and account elements do not decide which resource an ARN names:
/// every spelling addresses the one resource (appId, environmentName, id) names, for
/// reading and for writing alike.
#[tokio::test]
async fn the_partition_region_and_account_elements_do_not_decide_the_resource() {
    let router = emulator();

    for collection in COLLECTIONS {
        let created = create(
            &router,
            collection,
            APP_ID,
            ENVIRONMENT,
            "Addressed",
            Some(&json!({"env": "test"})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "the {collection} create must succeed, got {} with {:?}",
            created.status, created.body
        );
        let id = created.id();

        // The rows differ in which element is spelled differently from the emulator's own.
        let spellings: Vec<(&str, String)> = vec![
            (
                "another partition",
                arn_with(
                    "aws-cn",
                    REGION,
                    ACCOUNT,
                    collection,
                    APP_ID,
                    ENVIRONMENT,
                    &id,
                ),
            ),
            (
                "another region",
                arn_with(
                    "aws",
                    "us-east-1",
                    ACCOUNT,
                    collection,
                    APP_ID,
                    ENVIRONMENT,
                    &id,
                ),
            ),
            (
                "another account",
                arn_with(
                    "aws",
                    REGION,
                    "123456789012",
                    collection,
                    APP_ID,
                    ENVIRONMENT,
                    &id,
                ),
            ),
        ];

        for (case, resource_arn) in &spellings {
            let case = format!("{case} of a {collection} ARN");
            let listed = list_tags(&router, resource_arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: the resource must be reported, got {} with {:?}",
                listed.status, listed.body
            );
            assert_eq!(
                listed.tags(),
                json!({"env": "test"}),
                "case {case:?}: expected the stored tag set, got {:?}",
                listed.tags()
            );
        }

        // A write through one spelling is read back through the emulator's own, because
        // all of them name the one record.
        let written = tag(&router, &spellings[0].1, &json!({"team": "ui"})).await;
        assert_eq!(
            written.status, 200,
            "the write through another partition must succeed, got {} with {:?}",
            written.status, written.body
        );
        assert_tags(
            &format!("a write addressed through another partition of a {collection} ARN"),
            &router,
            collection,
            &id,
            &json!({"env": "test", "team": "ui"}),
        )
        .await;
    }
}
