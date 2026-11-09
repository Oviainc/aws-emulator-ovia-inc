// SPDX-License-Identifier: CC0-1.0

//! The wire shape of a tagged record's `tags` member, over HTTP, for EVERY resource kind
//! of this service that carries one.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: a resource of this service reports its tag set as a MAP of the
//! pairs it carries — the empty map when it carries none — so every reader of that one
//! record (the resource's own `Get…`, its `Export…`, and `ListTagsForResource` where an
//! ARN names it) reports the same set, whichever resource kind it is. `Component$tags`,
//! `Form$tags`, `Theme$tags` and `CodegenJob$tags` are the four members the rule covers:
//! `smithy_query(query="shape", shape="Component,Form,Theme,CodegenJob")` targets each of
//! them at `Tags` with no `required`, so the kinds are indistinguishable here and none of
//! them answers an absence.
//!
//! `resource_tagging.rs` owns the tagging surface itself (merging, removal, absent ARNs);
//! this suite is about the SHAPE the four kinds answer with, including after a removal
//! empties a set that a create never supplied.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment every case addresses.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The region and account elements the ARNs built here carry: the region the requests are
/// signed for, and the one account id a generated emulator presents
/// (`engine::emulator_account_id`).
const REGION: &str = "eu-west-1";
const ACCOUNT: &str = "000000000000";

/// The published bounds of a tag key and a tag value:
/// `smithy_query(query="shape", shape="Tags")` gives `TagKey` `@length(min: 1, max: 128)`
/// and `TagValue` `@length(min: 1, max: 256)`.
const TAG_KEY_MAX_LENGTH: usize = 128;
const TAG_VALUE_MAX_LENGTH: usize = 256;

/// One resource kind that carries a `Tags` member, named by the collection element of its
/// REST URIs (see this crate's `OPERATIONS.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Kind {
    /// The collection element of the kind's create and read URIs.
    collection: &'static str,
    /// Whether this kind has an `Export…` operation reporting whole records.
    exported: bool,
    /// Whether an ARN of this kind resolves on the tagging surface — the collection
    /// element is also the collection element of the documented ARN form for the three
    /// kinds `ListTagsForResource` reaches.
    tagged_by_arn: bool,
}

/// The four records of this service that carry a `Tags` member.
const KINDS: [Kind; 4] = [
    Kind {
        collection: "components",
        exported: true,
        tagged_by_arn: true,
    },
    Kind {
        collection: "forms",
        exported: true,
        tagged_by_arn: true,
    },
    Kind {
        collection: "themes",
        exported: true,
        tagged_by_arn: true,
    },
    Kind {
        collection: "codegen-jobs",
        exported: false,
        tagged_by_arn: false,
    },
];

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

    /// The `tags` member exactly as the wire carries it: `Value::Null` when the body omits
    /// it, which is what the rule forbids.
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

/// Percent-encode one value the way an SDK encodes a non-greedy `@httpLabel`, so a whole
/// ARN is ONE path segment.
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

/// The documented ARN form of one resource of this service.
fn arn(collection: &str, id: &str) -> String {
    format!(
        "arn:aws:amplifyuibuilder:{REGION}:{ACCOUNT}:app/{APP_ID}/environment/{ENVIRONMENT}/\
         {collection}/{id}"
    )
}

fn collection_uri(collection: &str) -> String {
    format!("/app/{APP_ID}/environment/{ENVIRONMENT}/{collection}")
}

/// The create payload of one resource kind, carrying only the members that kind's create
/// data requires, plus `tags` when the case supplies them.
fn create_payload(kind: Kind, name: &str, tags: Option<&Value>) -> Value {
    let mut data = match kind.collection {
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
    kind: Kind,
    name: &str,
    tags: Option<&Value>,
) -> Answer {
    call(
        router,
        "POST",
        &collection_uri(kind.collection),
        Some(create_payload(kind, name, tags)),
    )
    .await
}

async fn read_resource(router: &MultiServiceRouter, kind: Kind, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{}/{id}", collection_uri(kind.collection)),
        None,
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, kind: Kind, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/tags/{}", encoded(&arn(kind.collection, id))),
        None,
    )
    .await
}

async fn untag(router: &MultiServiceRouter, kind: Kind, id: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encoded(key)))
        .collect::<Vec<String>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", encoded(&arn(kind.collection, id))),
        None,
    )
    .await
}

async fn tag(router: &MultiServiceRouter, kind: Kind, id: &str, tags: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", encoded(&arn(kind.collection, id))),
        Some(json!({ "tags": tags })),
    )
    .await
}

/// The record one `Export…` page reports under `id`, or `None` when the export reports no
/// such record. The exports of this service are `@paginated(items: entities)`; the cases
/// here hold few enough records that one page carries them all.
async fn exported(router: &MultiServiceRouter, kind: Kind, id: &str) -> Option<Value> {
    let answer = call(
        router,
        "GET",
        &format!(
            "/export/app/{APP_ID}/environment/{ENVIRONMENT}/{}",
            kind.collection
        ),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the {} export must answer HTTP 200, got {} with {:?}",
        kind.collection, answer.status, answer.body
    );
    answer.body["entities"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "the {} export must carry an entities list, got {:?}",
                kind.collection, answer.body
            )
        })
        .iter()
        .find(|entity| entity["id"].as_str() == Some(id))
        .cloned()
}

/// Every reader of one record reports `expected` as its `tags` member.
async fn assert_every_reader_reports(
    case: &str,
    router: &MultiServiceRouter,
    kind: Kind,
    id: &str,
    expected: &Value,
) {
    let read = read_resource(router, kind, id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the resource must read back, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        &read.tags(),
        expected,
        "case {case:?}: the resource's own reader must report {expected:?}, got {:?}",
        read.tags()
    );

    if kind.exported {
        let entity = exported(router, kind, id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the export must report the record {id:?}"));
        let tags = entity.get("tags").cloned().unwrap_or(Value::Null);
        assert_eq!(
            &tags, expected,
            "case {case:?}: the export must report {expected:?}, got {tags:?}"
        );
    }

    if kind.tagged_by_arn {
        let listed = list_tags(router, kind, id).await;
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
    }
}

/// The tag sets a create supplies, differing along what the rule generalises over: whether
/// the create names `tags` at all, how many pairs it carries, the form of a key and of a
/// value, and each published length bound.
fn tag_set_rows() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no tags member at all", None),
        ("an empty tag set", Some(json!({}))),
        ("one pair", Some(json!({"env": "test"}))),
        (
            "three pairs",
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
            "a value carrying spaces, punctuation and non-ASCII text",
            Some(json!({"note": "Grüße, zum Test — 100%"})),
        ),
    ]
}

/// Whatever tag set a create supplied — including none at all — every reader of the record
/// reports it as a map, for every resource kind that carries one.
#[tokio::test]
async fn every_tagged_record_reports_its_tag_set_as_a_map() {
    let router = emulator();

    for kind in KINDS {
        for (index, (case, tags)) in tag_set_rows().into_iter().enumerate() {
            let case = format!("{case} on a {} record", kind.collection);
            let created = create(&router, kind, &format!("Shaped{index}"), tags.as_ref()).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the create must succeed, got {} with {:?}",
                created.status, created.body
            );

            // The empty map is a SET, so the create that stored none reports `{}` rather
            // than omitting the member.
            let expected = tags.unwrap_or_else(|| json!({}));
            assert_eq!(
                created.tags(),
                expected,
                "case {case:?}: the create must report {expected:?}, got {:?}",
                created.tags()
            );
            assert_every_reader_reports(&case, &router, kind, &created.id(), &expected).await;
        }
    }
}

/// A removal that empties the tag set of a record created without tags leaves the empty
/// map, not an absence: there is one stored set per record, and it is a map whatever it
/// holds.
#[tokio::test]
async fn a_removal_that_empties_a_tag_set_leaves_the_empty_map() {
    let router = emulator();

    for kind in KINDS.iter().filter(|kind| kind.tagged_by_arn) {
        let kind = *kind;
        let created = create(&router, kind, "Emptied", None).await;
        assert_eq!(
            created.status, 200,
            "the {} create must succeed, got {} with {:?}",
            kind.collection, created.status, created.body
        );
        let id = created.id();

        let written = tag(&router, kind, &id, &json!({"env": "test", "team": "ui"})).await;
        assert_eq!(
            written.status, 200,
            "the {} write must succeed, got {} with {:?}",
            kind.collection, written.status, written.body
        );
        assert_every_reader_reports(
            &format!("a write onto an untagged {} record", kind.collection),
            &router,
            kind,
            &id,
            &json!({"env": "test", "team": "ui"}),
        )
        .await;

        // The removals differ in how much of the set they take: one key, then the rest.
        for (case, keys, expected) in [
            ("one of two keys", vec!["team"], json!({"env": "test"})),
            ("the last key it carries", vec!["env"], json!({})),
            ("a key it no longer carries", vec!["env"], json!({})),
        ] {
            let case = format!("{case} of a {} record", kind.collection);
            let removed = untag(&router, kind, &id, &keys).await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: UntagResource must answer HTTP 200, got {} with {:?}",
                removed.status, removed.body
            );
            assert_every_reader_reports(&case, &router, kind, &id, &expected).await;
        }
    }
}
