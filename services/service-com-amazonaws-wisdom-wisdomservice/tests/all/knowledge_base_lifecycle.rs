// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Connect Wisdom knowledge-base slice
//! (`CreateKnowledgeBase` + `GetKnowledgeBase` + `DeleteKnowledgeBase`).
//!
//! Every assertion goes over the wire: a request is built, dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected. Nothing reaches into the
//! service's state struct, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * a create persists the knowledge base it mints, so a later read — by id or by ARN —
//!   answers out of the stored record, for every modelled `knowledgeBaseType` and every
//!   optional member the create accepts, and the `tags` member is reported for every
//!   knowledge base (empty when it carries none);
//! * a DELETE of a resource this service holds forgets that one record: the call answers
//!   the modelled 204, every later read of it — by any label form that used to name it —
//!   is `ResourceNotFoundException` (HTTP 404), a repeated delete is the same not-found,
//!   every other knowledge base is untouched, the name the record held is free for
//!   another create, and the `clientToken` that created it no longer names it;
//! * a label this account holds no knowledge base under — a bare id, an ARN of this
//!   account, an ARN of another account or partition — is that same not-found, while a
//!   label outside the modelled `UuidOrArn` pattern is `ValidationException` (HTTP 400).
//!
//! This view of the shared `wisdom` endpoint answers under its own registration key (see
//! `src/service.rs`), which is what every request below is signed for.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::state::MAX_KNOWLEDGE_BASES;
use service_com_amazonaws_wisdom_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for; the handler reads it out of the
/// SigV4 credential scope to build ARNs.
const REGION: &str = "ap-south-1";

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`): the shared `wisdom` key is served by the model that subsumes
/// this one.
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// A UUID no create ever mints, for the not-found assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The `KnowledgeBaseStatus` a knowledge base this service holds reports. Written out as
/// the literal rather than imported from the implementation, so the assertion pins the
/// modelled value (`smithy_query(query="shape",shape="KnowledgeBaseStatus")` declares it
/// beside the transient `CREATE_IN_PROGRESS`, `DELETE_IN_PROGRESS` and the rest).
const STATUS_ACTIVE: &str = "ACTIVE";

/// Every value of the modelled `KnowledgeBaseType` enum
/// (`smithy_query(query="shape",shape="KnowledgeBaseType")`), so the table spans the
/// whole catalog rather than the one type the issue's recording used.
const KNOWLEDGE_BASE_TYPES: [&str; 3] = ["EXTERNAL", "CUSTOM", "QUICK_RESPONSES"];

/// The published bounds of `name` and `description`
/// (`smithy_query(query="operation",operation="CreateKnowledgeBase")`: both `len
/// 1..255`).
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 255;

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    content_type: String,
    body: Value,
}

impl Reply {
    fn field(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer).filter(|v| !v.is_null())
    }

    fn string(&self, pointer: &str) -> Option<String> {
        self.field(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{ROUTE_KEY}/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("wisdom.{REGION}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let error_type = header("x-amzn-errortype");
        let content_type = header("content-type");
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            content_type,
            body,
        }
    }

    async fn create_knowledge_base(&self, body: Value) -> Reply {
        self.send("POST", "/knowledgeBases", Some(body)).await
    }

    async fn get_knowledge_base(&self, label: &str) -> Reply {
        // An ARN label carries `/`, which must not be read as path structure.
        let label = label.replace('/', "%2F");
        self.send("GET", &format!("/knowledgeBases/{label}"), None)
            .await
    }

    async fn delete_knowledge_base(&self, label: &str) -> Reply {
        let label = label.replace('/', "%2F");
        self.send("DELETE", &format!("/knowledgeBases/{label}"), None)
            .await
    }
}

/// The knowledge-base ARN AWS documents for this service:
/// `arn:aws:wisdom:<region>:<account>:knowledge-base/<knowledgeBaseId>`. Written out
/// here rather than taken from the handler's helper, so the test pins the format.
fn expected_knowledge_base_arn(region: &str, knowledge_base_id: &str) -> String {
    format!(
        "arn:aws:wisdom:{region}:{account}:knowledge-base/{knowledge_base_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// Whether `value` is a lowercase hyphenated UUID, the form the model's `UuidOrArn`
/// pattern requires (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
fn is_modelled_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
}

/// One row of the create/read/delete table: what varies between knowledge bases.
#[derive(Debug, Clone)]
struct KnowledgeBaseCase {
    label: &'static str,
    name: String,
    knowledge_base_type: &'static str,
    description: Option<String>,
    tags: BTreeMap<String, String>,
    rendering_template_uri: Option<&'static str>,
    kms_key_id: Option<&'static str>,
    app_integration_arn: Option<&'static str>,
}

impl KnowledgeBaseCase {
    fn create_body(&self) -> Value {
        let mut body = json!({ "name": self.name, "knowledgeBaseType": self.knowledge_base_type });
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        if let Some(template_uri) = self.rendering_template_uri {
            body["renderingConfiguration"] = json!({ "templateUri": template_uri });
        }
        if let Some(kms_key_id) = self.kms_key_id {
            body["serverSideEncryptionConfiguration"] = json!({ "kmsKeyId": kms_key_id });
        }
        if let Some(app_integration_arn) = self.app_integration_arn {
            body["sourceConfiguration"] =
                json!({ "appIntegrations": { "appIntegrationArn": app_integration_arn } });
        }
        body
    }
}

/// The table: one row per axis the rules generalise over — the knowledge-base type
/// (every modelled value), the name's form and length (both published bounds), the
/// presence of a description, the tag count, and each optional configuration member the
/// create accepts.
fn knowledge_base_cases() -> Vec<KnowledgeBaseCase> {
    let mut cases = vec![
        // The shape of the case the issue's recording used: a named CUSTOM knowledge
        // base with tags.
        KnowledgeBaseCase {
            label: "named CUSTOM base with one tag",
            name: "wisdom-kb-1".to_string(),
            knowledge_base_type: "CUSTOM",
            description: Some("a knowledge base, described".to_string()),
            tags: [("env".to_string(), "test".to_string())]
                .into_iter()
                .collect(),
            rendering_template_uri: None,
            kms_key_id: None,
            app_integration_arn: None,
        },
        KnowledgeBaseCase {
            label: "shortest legal name, no description, no tags",
            name: "k".repeat(NAME_MIN),
            knowledge_base_type: "CUSTOM",
            description: None,
            tags: BTreeMap::new(),
            rendering_template_uri: None,
            kms_key_id: None,
            app_integration_arn: None,
        },
        KnowledgeBaseCase {
            label: "longest legal name and description, several tags, external source",
            name: "k".repeat(NAME_MAX),
            knowledge_base_type: "EXTERNAL",
            description: Some("d".repeat(NAME_MAX)),
            tags: [
                ("a.b".to_string(), "1".to_string()),
                ("team_1".to_string(), "alpha".to_string()),
                ("p-q".to_string(), "2".to_string()),
            ]
            .into_iter()
            .collect(),
            rendering_template_uri: Some("https://example.test/articles/${Id}"),
            kms_key_id: None,
            app_integration_arn: Some(
                "arn:aws:app-integrations:ap-south-1:000000000000:data-integration/kb-source",
            ),
        },
        KnowledgeBaseCase {
            label: "mixed-case name with the punctuation the pattern admits, encrypted",
            name: "Mixed Case_kb.9,-".to_string(),
            knowledge_base_type: "CUSTOM",
            description: None,
            tags: BTreeMap::new(),
            rendering_template_uri: None,
            kms_key_id: Some("arn:aws:kms:ap-south-1:000000000000:key/abcd1234"),
            app_integration_arn: None,
        },
    ];
    // Every modelled type, so the catalog is covered and not only the two types the rows
    // above happen to use.
    for (index, knowledge_base_type) in KNOWLEDGE_BASE_TYPES.into_iter().enumerate() {
        cases.push(KnowledgeBaseCase {
            label: "one row per modelled knowledgeBaseType",
            name: format!("typed-kb-{index}-{knowledge_base_type}").replace('_', "-"),
            knowledge_base_type,
            description: None,
            tags: BTreeMap::new(),
            rendering_template_uri: None,
            kms_key_id: None,
            app_integration_arn: None,
        });
    }
    cases
}

/// Every member of the reported knowledge base, checked against the row that created it.
fn assert_reported_knowledge_base(
    case: &KnowledgeBaseCase,
    reply: &Reply,
    knowledge_base_id: &str,
    source: &str,
) {
    let label = case.label;
    assert_eq!(
        reply.status, 200,
        "case {label:?} ({source}): expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.content_type, "application/json",
        "case {label:?} ({source}): restJson1 answers application/json, got {:?}",
        reply.content_type
    );
    assert_eq!(
        reply.string("/knowledgeBase/knowledgeBaseId").as_deref(),
        Some(knowledge_base_id),
        "case {label:?} ({source}): expected the stored knowledgeBaseId, got {:?}",
        reply.string("/knowledgeBase/knowledgeBaseId")
    );
    assert_eq!(
        reply.string("/knowledgeBase/knowledgeBaseArn"),
        Some(expected_knowledge_base_arn(REGION, knowledge_base_id)),
        "case {label:?} ({source}): expected the knowledge-base ARN of the signed region, \
         got {:?}",
        reply.string("/knowledgeBase/knowledgeBaseArn")
    );
    assert_eq!(
        reply.string("/knowledgeBase/name").as_deref(),
        Some(case.name.as_str()),
        "case {label:?} ({source}): expected the stored name, got {:?}",
        reply.string("/knowledgeBase/name")
    );
    assert_eq!(
        reply.string("/knowledgeBase/knowledgeBaseType").as_deref(),
        Some(case.knowledge_base_type),
        "case {label:?} ({source}): expected the stored knowledgeBaseType, got {:?}",
        reply.string("/knowledgeBase/knowledgeBaseType")
    );
    assert_eq!(
        reply.string("/knowledgeBase/status").as_deref(),
        Some(STATUS_ACTIVE),
        "case {label:?} ({source}): a knowledge base this service holds is {STATUS_ACTIVE}, \
         got {:?}",
        reply.string("/knowledgeBase/status")
    );
    assert_eq!(
        reply.string("/knowledgeBase/description"),
        case.description.clone(),
        "case {label:?} ({source}): expected the stored description, got {:?}",
        reply.string("/knowledgeBase/description")
    );
    let reported_tags = reply.field("/knowledgeBase/tags").cloned();
    let expected_tags = Some(json!(case.tags));
    assert_eq!(
        reported_tags, expected_tags,
        "case {label:?} ({source}): expected the stored tags {expected_tags:?} (the `tags` \
         member is reported for every knowledge base, empty when it carries none), got \
         {reported_tags:?}"
    );
    let reported_template = reply.string("/knowledgeBase/renderingConfiguration/templateUri");
    assert_eq!(
        reported_template.as_deref(),
        case.rendering_template_uri,
        "case {label:?} ({source}): expected the stored templateUri {:?}, got \
         {reported_template:?}",
        case.rendering_template_uri
    );
    let reported_key = reply.string("/knowledgeBase/serverSideEncryptionConfiguration/kmsKeyId");
    assert_eq!(
        reported_key.as_deref(),
        case.kms_key_id,
        "case {label:?} ({source}): expected the stored kmsKeyId {:?}, got {reported_key:?}",
        case.kms_key_id
    );
    let reported_source =
        reply.string("/knowledgeBase/sourceConfiguration/appIntegrations/appIntegrationArn");
    assert_eq!(
        reported_source.as_deref(),
        case.app_integration_arn,
        "case {label:?} ({source}): expected the stored appIntegrationArn {:?}, got \
         {reported_source:?}",
        case.app_integration_arn
    );
    assert_eq!(
        reply.field("/knowledgeBase/lastContentModificationTime"),
        None,
        "case {label:?} ({source}): a knowledge base with no content reports no \
         lastContentModificationTime, got {}",
        reply.body
    );
}

/// A create persists what it mints: every member the create accepted comes back out of
/// the STORED record, both from the create's own answer and from a later read addressed
/// by id and by ARN.
#[tokio::test]
async fn a_created_knowledge_base_reads_back_by_id_and_by_arn() {
    let emulator = Emulator::new();

    for case in knowledge_base_cases() {
        let label = case.label;
        let created = emulator.create_knowledge_base(case.create_body()).await;
        let knowledge_base_id = created
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {label:?}: no knowledgeBaseId in {}", created.body));
        assert!(
            is_modelled_uuid(&knowledge_base_id),
            "case {label:?}: expected a lowercase-hex UUID knowledgeBaseId, got \
             {knowledge_base_id:?}"
        );
        assert_reported_knowledge_base(&case, &created, &knowledge_base_id, "CreateKnowledgeBase");

        let by_id = emulator.get_knowledge_base(&knowledge_base_id).await;
        assert_reported_knowledge_base(&case, &by_id, &knowledge_base_id, "GetKnowledgeBase by id");

        let arn = expected_knowledge_base_arn(REGION, &knowledge_base_id);
        let by_arn = emulator.get_knowledge_base(&arn).await;
        assert_reported_knowledge_base(
            &case,
            &by_arn,
            &knowledge_base_id,
            "GetKnowledgeBase by ARN",
        );
    }
}

/// A delete removes the knowledge base from the store: it answers the modelled empty
/// 204, and from then on every read of that knowledge base — by any label form that used
/// to name it — is the not-found its own operation models, as is a second delete of it.
#[tokio::test]
async fn a_deleted_knowledge_base_is_gone() {
    /// How the delete addresses the knowledge base it removes.
    #[derive(Debug, Clone, Copy)]
    enum Addressed {
        Id,
        Arn,
    }

    for case in knowledge_base_cases() {
        for addressed in [Addressed::Id, Addressed::Arn] {
            let label = case.label;
            // Each pass gets its own store, so a row's name (one row's is the longest
            // the model admits, which leaves no room for a suffix) is free every time.
            let emulator = Emulator::new();
            let created = emulator.create_knowledge_base(case.create_body()).await;
            let knowledge_base_id = created
                .string("/knowledgeBase/knowledgeBaseId")
                .unwrap_or_else(|| {
                    panic!(
                        "case {label:?} ({addressed:?}): no knowledgeBaseId in {}",
                        created.body
                    )
                });
            let arn = expected_knowledge_base_arn(REGION, &knowledge_base_id);
            let addressed_label = match addressed {
                Addressed::Id => knowledge_base_id.clone(),
                Addressed::Arn => arn.clone(),
            };

            let deleted = emulator.delete_knowledge_base(&addressed_label).await;
            assert_eq!(
                deleted.status, 204,
                "case {label:?} ({addressed:?}): expected the modelled HTTP 204 for a delete, \
                 got {} with body {}",
                deleted.status, deleted.body
            );
            assert!(
                deleted
                    .body
                    .as_object()
                    .is_none_or(serde_json::Map::is_empty),
                "case {label:?} ({addressed:?}): DeleteKnowledgeBase models an empty output, \
                 got {}",
                deleted.body
            );

            for (form, addressed_again) in [("by id", &knowledge_base_id), ("by ARN", &arn)] {
                let read_back = emulator.get_knowledge_base(addressed_again).await;
                assert_eq!(
                    read_back.status, 404,
                    "case {label:?} ({addressed:?}, read back {form}): expected HTTP 404 after \
                     the delete, got {} with body {}",
                    read_back.status, read_back.body
                );
                assert!(
                    read_back.error_type.contains("ResourceNotFoundException"),
                    "case {label:?} ({addressed:?}, read back {form}): expected \
                     ResourceNotFoundException, got {:?} with body {}",
                    read_back.error_type,
                    read_back.body
                );
            }

            let again = emulator.delete_knowledge_base(&addressed_label).await;
            assert_eq!(
                again.status, 404,
                "case {label:?} ({addressed:?}): expected HTTP 404 for a repeated delete, got \
                 {} with body {}",
                again.status, again.body
            );
            assert!(
                again.error_type.contains("ResourceNotFoundException"),
                "case {label:?} ({addressed:?}): expected ResourceNotFoundException for a \
                 repeated delete, got {:?} with body {}",
                again.error_type,
                again.body
            );
        }
    }
}

/// A delete removes exactly the knowledge base it addresses: every other knowledge base
/// the account holds still reads back unchanged, whichever one of them is deleted.
#[tokio::test]
async fn a_delete_leaves_every_other_knowledge_base_alone() {
    let emulator = Emulator::new();

    // Rows differ in WHICH of the stored knowledge bases the delete addresses — the
    // first, a middle one and the last — so the rule is not tested on one position only.
    for (label, deleted_index) in [("the first", 0usize), ("a middle one", 2), ("the last", 4)] {
        let mut ids = Vec::new();
        for index in 0..5 {
            let created = emulator
                .create_knowledge_base(json!({
                    "name": format!("neighbour-kb-{label}-{index}").replace(' ', "-"),
                    "knowledgeBaseType": KNOWLEDGE_BASE_TYPES[index % KNOWLEDGE_BASE_TYPES.len()],
                }))
                .await;
            ids.push(
                created
                    .string("/knowledgeBase/knowledgeBaseId")
                    .unwrap_or_else(|| {
                        panic!("case {label:?}: no knowledgeBaseId in {}", created.body)
                    }),
            );
        }

        let target = ids[deleted_index].clone();
        let deleted = emulator.delete_knowledge_base(&target).await;
        assert_eq!(
            deleted.status, 204,
            "case {label:?}: expected HTTP 204 for the delete, got {} with body {}",
            deleted.status, deleted.body
        );

        for (index, id) in ids.iter().enumerate() {
            let reply = emulator.get_knowledge_base(id).await;
            let expected_status = if index == deleted_index { 404 } else { 200 };
            assert_eq!(
                reply.status, expected_status,
                "case {label:?} (neighbour {index}): expected HTTP {expected_status} after \
                 deleting index {deleted_index}, got {} with body {}",
                reply.status, reply.body
            );
            if expected_status == 200 {
                assert_eq!(
                    reply.string("/knowledgeBase/knowledgeBaseId").as_deref(),
                    Some(id.as_str()),
                    "case {label:?} (neighbour {index}): expected the stored record to be \
                     untouched, got {}",
                    reply.body
                );
            }
        }
    }
}

/// The name a deleted knowledge base held is free again: the conflict a duplicate name
/// is refused with is a conflict with a LIVE knowledge base, so a create after the
/// delete succeeds and mints a DIFFERENT knowledge base.
#[tokio::test]
async fn a_delete_releases_the_name_it_held() {
    let emulator = Emulator::new();

    // Rows differ in the name's form and in the type the second create asks for, so the
    // release is shown to be about the name alone.
    for (label, name, first_type, second_type) in [
        (
            "plain name, same type",
            "recycled-kb".to_string(),
            "CUSTOM",
            "CUSTOM",
        ),
        (
            "shortest legal name",
            "k".repeat(NAME_MIN),
            "CUSTOM",
            "EXTERNAL",
        ),
        (
            "longest legal name",
            "k".repeat(NAME_MAX),
            "EXTERNAL",
            "QUICK_RESPONSES",
        ),
        (
            "name with the punctuation the pattern admits",
            "Recycled Case_kb.9,-".to_string(),
            "QUICK_RESPONSES",
            "CUSTOM",
        ),
    ] {
        let first = emulator
            .create_knowledge_base(json!({ "name": name, "knowledgeBaseType": first_type }))
            .await;
        let first_id = first
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {label:?}: no knowledgeBaseId in {}", first.body));

        // While it is live, the name is taken.
        let refused = emulator
            .create_knowledge_base(json!({ "name": name, "knowledgeBaseType": second_type }))
            .await;
        assert_eq!(
            refused.status, 409,
            "case {label:?}: expected HTTP 409 while the name is held by a live knowledge \
             base, got {} with body {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ConflictException"),
            "case {label:?}: expected ConflictException, got {:?} with body {}",
            refused.error_type,
            refused.body
        );

        let deleted = emulator.delete_knowledge_base(&first_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {label:?}: expected HTTP 204 for the delete, got {} with body {}",
            deleted.status, deleted.body
        );

        let second = emulator
            .create_knowledge_base(json!({ "name": name, "knowledgeBaseType": second_type }))
            .await;
        assert_eq!(
            second.status, 200,
            "case {label:?}: expected the name to be free after the delete, got {} with body {}",
            second.status, second.body
        );
        let second_id = second
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {label:?}: no knowledgeBaseId in {}", second.body));
        assert_ne!(
            second_id, first_id,
            "case {label:?}: a create after a delete mints a NEW knowledge base, got the \
             deleted id {first_id} again"
        );
        assert_eq!(
            second.string("/knowledgeBase/knowledgeBaseType").as_deref(),
            Some(second_type),
            "case {label:?}: expected the second create's type to be stored, got {}",
            second.body
        );

        let read_back = emulator.get_knowledge_base(&second_id).await;
        assert_eq!(
            read_back.status, 200,
            "case {label:?}: expected the replacement to read back, got {} with body {}",
            read_back.status, read_back.body
        );
        let old = emulator.get_knowledge_base(&first_id).await;
        assert_eq!(
            old.status, 404,
            "case {label:?}: expected the deleted knowledge base to stay gone, got {} with \
             body {}",
            old.status, old.body
        );
    }
}

/// A `clientToken` whose knowledge base was deleted does not resurrect it: the token is
/// released with the record, so a create replaying it mints a new knowledge base rather
/// than reporting the deleted one — while a replay of a token whose knowledge base is
/// still live answers with that one.
#[tokio::test]
async fn a_client_token_names_only_a_live_knowledge_base() {
    let emulator = Emulator::new();

    for (label, token) in [
        ("short token", "d".to_string()),
        (
            "uuid-shaped token",
            "1a2b3c4d-5e6f-4071-8293-a4b5c6d7e8f9".to_string(),
        ),
        ("longest legal token", "d".repeat(4096)),
    ] {
        let body = json!({
            "name": format!("token-kb-{label}").replace(' ', "-"),
            "knowledgeBaseType": "CUSTOM",
            "clientToken": token,
        });
        let first = emulator.create_knowledge_base(body.clone()).await;
        let first_id = first
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {label:?}: no knowledgeBaseId in {}", first.body));

        // While the knowledge base is live, the token replays onto it.
        let replayed = emulator.create_knowledge_base(body.clone()).await;
        assert_eq!(
            replayed.string("/knowledgeBase/knowledgeBaseId").as_deref(),
            Some(first_id.as_str()),
            "case {label:?}: expected the replay to answer with {first_id}, got {} with body {}",
            replayed.status,
            replayed.body
        );

        let deleted = emulator.delete_knowledge_base(&first_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {label:?}: expected HTTP 204 for the delete, got {} with body {}",
            deleted.status, deleted.body
        );

        let after_delete = emulator.create_knowledge_base(body).await;
        assert_eq!(
            after_delete.status, 200,
            "case {label:?}: expected the create to succeed after the delete, got {} with \
             body {}",
            after_delete.status, after_delete.body
        );
        let new_id = after_delete
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: no knowledgeBaseId in {}",
                    after_delete.body
                )
            });
        assert_ne!(
            new_id, first_id,
            "case {label:?}: a replay of a token whose knowledge base is deleted mints a new \
             one, got the deleted id {first_id} again"
        );

        let read_back = emulator.get_knowledge_base(&new_id).await;
        assert_eq!(
            read_back.status, 200,
            "case {label:?}: expected the knowledge base the replay minted to read back, got \
             {} with body {}",
            read_back.status, read_back.body
        );
    }
}

/// A read or a delete addressed to a label this account holds no knowledge base under is
/// `ResourceNotFoundException` (HTTP 404) and changes nothing; a label outside the
/// modelled `UuidOrArn` pattern never reaches the store at all (HTTP 400).
#[tokio::test]
async fn an_unknown_knowledge_base_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .create_knowledge_base(json!({ "name": "survivor-kb", "knowledgeBaseType": "CUSTOM" }))
        .await;
    let live_id = created
        .string("/knowledgeBase/knowledgeBaseId")
        .unwrap_or_else(|| panic!("no knowledgeBaseId in {}", created.body));

    let absent: Vec<(&str, String)> = vec![
        ("bare uuid nothing holds", ABSENT_UUID.to_string()),
        (
            "this account's arn for an absent id",
            expected_knowledge_base_arn(REGION, ABSENT_UUID),
        ),
        (
            "another account's arn for a live id",
            format!("arn:aws:wisdom:{REGION}:123456789012:knowledge-base/{live_id}"),
        ),
        (
            "another partition's arn for a live id",
            format!(
                "arn:aws-cn:wisdom:{REGION}:{}:knowledge-base/{live_id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];

    for (label, addressed) in &absent {
        for (operation, reply) in [
            ("GetKnowledgeBase", emulator.get_knowledge_base(addressed).await),
            (
                "DeleteKnowledgeBase",
                emulator.delete_knowledge_base(addressed).await,
            ),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {label:?} ({operation}): expected HTTP 404, got {} with body {}",
                reply.status, reply.body
            );
            assert!(
                reply.error_type.contains("ResourceNotFoundException"),
                "case {label:?} ({operation}): expected ResourceNotFoundException, got {:?} \
                 with body {}",
                reply.error_type,
                reply.body
            );
            assert_eq!(
                reply.content_type, "application/json",
                "case {label:?} ({operation}): restJson1 answers application/json even for an \
                 error, got {:?}",
                reply.content_type
            );
        }
    }

    // A label outside the modelled `UuidOrArn` pattern is refused by the model's own
    // constraint, before any lookup. The pattern pins the service segment to `wisdom`,
    // so another service's ARN is such a label
    // (`smithy_query(query="operation",operation="DeleteKnowledgeBase")`).
    let malformed: Vec<(&str, String)> = vec![
        (
            "another service's arn for a live id",
            format!(
                "arn:aws:connect:{REGION}:{}:knowledge-base/{live_id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        ("not an id at all", "not-a-knowledge-base".to_string()),
    ];
    for (label, addressed) in &malformed {
        for (operation, reply) in [
            ("GetKnowledgeBase", emulator.get_knowledge_base(addressed).await),
            (
                "DeleteKnowledgeBase",
                emulator.delete_knowledge_base(addressed).await,
            ),
        ] {
            assert_eq!(
                reply.status, 400,
                "case {label:?} ({operation}): expected HTTP 400 for a label outside the \
                 modelled pattern, got {} with body {}",
                reply.status, reply.body
            );
            assert!(
                reply.error_type.contains("ValidationException"),
                "case {label:?} ({operation}): expected ValidationException, got {:?} with \
                 body {}",
                reply.error_type,
                reply.body
            );
        }
    }

    // None of those refusals touched the knowledge base this account does hold.
    let still_there = emulator.get_knowledge_base(&live_id).await;
    assert_eq!(
        still_there.status, 200,
        "case \"live knowledge base after refused calls\": expected HTTP 200, got {} with \
         body {}",
        still_there.status, still_there.body
    );
}

/// The store is bounded: a create past the account's ceiling is refused with the
/// modelled `ServiceQuotaExceededException` (HTTP 402), the ceiling is on OCCUPANCY —
/// every create up to it succeeds — and a delete frees a slot, so a create after one
/// succeeds again.
#[tokio::test]
async fn the_knowledge_base_store_is_bounded() {
    let emulator = Emulator::new();

    let mut first_id = String::new();
    for index in 0..MAX_KNOWLEDGE_BASES {
        let reply = emulator
            .create_knowledge_base(
                json!({ "name": format!("bounded-kb-{index}"), "knowledgeBaseType": "CUSTOM" }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {index}: expected HTTP 200 inside the bound, got {} with body {}",
            reply.status, reply.body
        );
        if index == 0 {
            first_id = reply
                .string("/knowledgeBase/knowledgeBaseId")
                .unwrap_or_else(|| panic!("case {index}: no knowledgeBaseId in {}", reply.body));
        }
    }

    let refused = emulator
        .create_knowledge_base(json!({ "name": "one-too-many", "knowledgeBaseType": "CUSTOM" }))
        .await;
    assert_eq!(
        refused.status, 402,
        "case \"past the bound\": expected HTTP 402, got {} with body {}",
        refused.status, refused.body
    );
    assert!(
        refused.error_type.contains("ServiceQuotaExceededException"),
        "case \"past the bound\": expected ServiceQuotaExceededException, got {:?} with body {}",
        refused.error_type,
        refused.body
    );

    // A delete releases the slot the record occupied, so the next create fits.
    let deleted = emulator.delete_knowledge_base(&first_id).await;
    assert_eq!(
        deleted.status, 204,
        "case \"delete to free a slot\": expected HTTP 204, got {} with body {}",
        deleted.status, deleted.body
    );
    let accepted = emulator
        .create_knowledge_base(json!({ "name": "one-after-a-delete", "knowledgeBaseType": "CUSTOM" }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"after a delete freed a slot\": expected HTTP 200, got {} with body {}",
        accepted.status, accepted.body
    );
}
