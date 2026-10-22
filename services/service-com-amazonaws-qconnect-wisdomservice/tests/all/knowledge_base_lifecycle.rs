// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Q in Connect knowledge-base slice
//! (`CreateKnowledgeBase` + `GetKnowledgeBase`).
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
//!   optional member the create accepts;
//! * the `tags` member is reported for every knowledge base, empty when it carries none;
//! * a name a live knowledge base already holds is `ConflictException` (HTTP 409), and
//!   the first knowledge base is untouched by the refusal;
//! * a replayed `clientToken` answers with the knowledge base that token already
//!   created rather than minting a second one;
//! * a label this account holds no knowledge base under — a bare id, an ARN of this
//!   account, an ARN of another account or partition — is `ResourceNotFoundException`
//!   (HTTP 404).
//!
//! The harness below is this suite's own: the sibling `assistant_lifecycle` suite's one
//! is private to that module and speaks in assistants.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::state::MAX_KNOWLEDGE_BASES;
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for; the handler reads it out of the
/// SigV4 credential scope to build ARNs.
const REGION: &str = "eu-west-1";

/// A UUID no create ever mints, for the not-found assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The `KnowledgeBaseStatus` a knowledge base the service holds reports. Written out as
/// the literal the recorded `CreateKnowledgeBase` answer carries, NOT imported from the
/// implementation, so the assertion pins the modelled value rather than whatever constant
/// the handler happens to use
/// (`smithy_query(query="shape",shape="KnowledgeBaseStatus")` declares it).
const STATUS_ACTIVE: &str = "ACTIVE";

/// Every value of the modelled `KnowledgeBaseType` enum
/// (`smithy_query(query="shape",shape="KnowledgeBaseType")`), so the round-trip table
/// spans the whole catalog rather than the one type the issue's recording used.
const KNOWLEDGE_BASE_TYPES: [&str; 5] = [
    "CUSTOM",
    "EXTERNAL",
    "QUICK_RESPONSES",
    "MESSAGE_TEMPLATES",
    "MANAGED",
];

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
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/wisdom/aws4_request, \
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

/// Whether `value` is a lowercase hyphenated UUID, the form the model's `Uuid` pattern
/// requires (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
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

/// One row of the create/read table: what varies between knowledge bases.
#[derive(Debug, Clone)]
struct KnowledgeBaseCase {
    label: &'static str,
    name: String,
    knowledge_base_type: &'static str,
    description: Option<String>,
    tags: BTreeMap<String, String>,
    rendering_template_uri: Option<&'static str>,
    kms_key_id: Option<&'static str>,
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
        body
    }
}

/// The table: one row per axis the create rule generalises over — the knowledge-base
/// type (every modelled value), the name's form and length (both published bounds), the
/// presence of a description, the tag count, and each optional configuration member.
fn knowledge_base_cases() -> Vec<KnowledgeBaseCase> {
    let mut cases = vec![
        // The shape of the case the issue's recording used: a named CUSTOM knowledge
        // base with tags.
        KnowledgeBaseCase {
            label: "named CUSTOM base with one tag",
            name: "sim2real-kb-1".to_string(),
            knowledge_base_type: "CUSTOM",
            description: Some("a knowledge base, described".to_string()),
            tags: [("env".to_string(), "test".to_string())]
                .into_iter()
                .collect(),
            rendering_template_uri: None,
            kms_key_id: None,
        },
        KnowledgeBaseCase {
            label: "shortest legal name, no description, no tags",
            name: "k".repeat(NAME_MIN),
            knowledge_base_type: "CUSTOM",
            description: None,
            tags: BTreeMap::new(),
            rendering_template_uri: None,
            kms_key_id: None,
        },
        KnowledgeBaseCase {
            label: "longest legal name and description, several tags",
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
        },
        KnowledgeBaseCase {
            label: "mixed-case name with the punctuation the pattern admits, encrypted",
            name: "Mixed Case_kb.9,-".to_string(),
            knowledge_base_type: "CUSTOM",
            description: None,
            tags: BTreeMap::new(),
            rendering_template_uri: None,
            kms_key_id: Some("arn:aws:kms:eu-west-1:000000000000:key/abcd1234"),
        },
    ];
    // Every remaining modelled type, so the catalog is covered and not only the two
    // types the rows above happen to use.
    for (index, knowledge_base_type) in KNOWLEDGE_BASE_TYPES.into_iter().enumerate() {
        cases.push(KnowledgeBaseCase {
            label: "one row per modelled knowledgeBaseType",
            name: format!("typed-kb-{index}-{knowledge_base_type}").replace('_', "-"),
            knowledge_base_type,
            description: None,
            tags: BTreeMap::new(),
            rendering_template_uri: None,
            kms_key_id: None,
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

/// A name a live knowledge base already holds is the modelled conflict (HTTP 409), and
/// the knowledge base holding that name is untouched by the refusal.
#[tokio::test]
async fn a_duplicate_knowledge_base_name_is_a_conflict() {
    let emulator = Emulator::new();

    // Rows differ in the type and in the optional members, so the conflict is shown to
    // key on the NAME alone and not on the rest of the request.
    for (label, first, second) in [
        (
            "same name, same type",
            json!({ "name": "duplicate-kb", "knowledgeBaseType": "CUSTOM" }),
            json!({ "name": "duplicate-kb", "knowledgeBaseType": "CUSTOM" }),
        ),
        (
            "same name, another type",
            json!({ "name": "duplicate-kb-2", "knowledgeBaseType": "CUSTOM" }),
            json!({ "name": "duplicate-kb-2", "knowledgeBaseType": "QUICK_RESPONSES" }),
        ),
        (
            "same name, extra members on the second create",
            json!({ "name": "duplicate-kb-3", "knowledgeBaseType": "EXTERNAL" }),
            json!({
                "name": "duplicate-kb-3",
                "knowledgeBaseType": "EXTERNAL",
                "description": "second try",
                "tags": { "env": "test" }
            }),
        ),
    ] {
        let created = emulator.create_knowledge_base(first).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected the first create to succeed, got {} with body {}",
            created.status, created.body
        );
        let knowledge_base_id = created
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {label:?}: no knowledgeBaseId in {}", created.body));

        let refused = emulator.create_knowledge_base(second).await;
        assert_eq!(
            refused.status, 409,
            "case {label:?}: expected HTTP 409 for a name already in use, got {} with body {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ConflictException"),
            "case {label:?}: expected ConflictException, got {:?} with body {}",
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.content_type, "application/json",
            "case {label:?}: restJson1 answers application/json even for an error, got {:?}",
            refused.content_type
        );
        assert_eq!(
            refused.field("/knowledgeBase"),
            None,
            "case {label:?}: a refused create reports no knowledge base, got {}",
            refused.body
        );

        // The refusal minted nothing and changed nothing: the live knowledge base still
        // reads back, and it is still the one the first create made.
        let read_back = emulator.get_knowledge_base(&knowledge_base_id).await;
        assert_eq!(
            read_back.string("/knowledgeBase/knowledgeBaseId").as_deref(),
            Some(knowledge_base_id.as_str()),
            "case {label:?}: expected the first knowledge base to survive the conflict, got \
             {} with body {}",
            read_back.status,
            read_back.body
        );
    }
}

/// A replayed `clientToken` answers with the knowledge base that token already created,
/// rather than minting a second one — for any token form, and independently per token.
#[tokio::test]
async fn a_replayed_client_token_creates_one_knowledge_base() {
    let emulator = Emulator::new();

    for (label, token) in [
        ("short token", "t".to_string()),
        ("uuid-shaped token", "7f2d1c9e-0b3a-4c5d-8e6f-112233445566".to_string()),
        ("longest legal token", "t".repeat(4096)),
    ] {
        let body = json!({
            "name": format!("idempotent-kb-{label}").replace(' ', "-"),
            "knowledgeBaseType": "CUSTOM",
            "clientToken": token,
        });
        let first = emulator.create_knowledge_base(body.clone()).await;
        let knowledge_base_id = first
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {label:?}: no knowledgeBaseId in {}", first.body));

        let replayed = emulator.create_knowledge_base(body).await;
        assert_eq!(
            replayed.status, 200,
            "case {label:?}: expected the replay to answer 200, got {} with body {}",
            replayed.status, replayed.body
        );
        assert_eq!(
            replayed
                .string("/knowledgeBase/knowledgeBaseId")
                .as_deref(),
            Some(knowledge_base_id.as_str()),
            "case {label:?}: expected the replay to name {knowledge_base_id}, got {:?}",
            replayed.string("/knowledgeBase/knowledgeBaseId")
        );
    }
}

/// A label this account holds no knowledge base under is `ResourceNotFoundException`
/// (HTTP 404), whichever legal label form names it — including an ARN this emulator
/// could not have minted, which names no resource even when its trailing UUID is a live
/// knowledge base's.
#[tokio::test]
async fn an_unknown_knowledge_base_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .create_knowledge_base(json!({ "name": "addressed-kb", "knowledgeBaseType": "CUSTOM" }))
        .await;
    let live_id = created
        .string("/knowledgeBase/knowledgeBaseId")
        .unwrap_or_else(|| panic!("no knowledgeBaseId in {}", created.body));

    for (label, addressed) in [
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
    ] {
        let reply = emulator.get_knowledge_base(&addressed).await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.content_type, "application/json",
            "case {label:?}: restJson1 answers application/json even for an error, got {:?}",
            reply.content_type
        );
    }

    // And the knowledge base those foreign ARNs named is untouched: a read of it still
    // answers out of the stored record.
    let still_there = emulator.get_knowledge_base(&live_id).await;
    assert_eq!(
        still_there.status, 200,
        "case \"live knowledge base after foreign-ARN addressing\": expected HTTP 200, got {} \
         with body {}",
        still_there.status, still_there.body
    );

    // A label outside the modelled `UuidOrArn` pattern never reaches the lookup at all:
    // the model's own constraint refuses it with `ValidationException` (HTTP 400). The
    // pattern pins the service segment to `wisdom`, so another service's ARN is such a
    // label (`smithy_query(query="operation",operation="GetKnowledgeBase")`).
    for (label, addressed) in [
        (
            "another service's arn for a live id",
            format!(
                "arn:aws:connect:{REGION}:{}:knowledge-base/{live_id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        ("not an id at all", "not-a-knowledge-base".to_string()),
    ] {
        let reply = emulator.get_knowledge_base(&addressed).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400 for a label outside the modelled pattern, got \
             {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ValidationException"),
            "case {label:?}: expected ValidationException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
    }
}

/// The store is bounded: a create past the account's ceiling is refused with the
/// modelled `ServiceQuotaExceededException` (HTTP 402), and the ceiling is on OCCUPANCY
/// — every create up to it succeeds.
#[tokio::test]
async fn the_knowledge_base_store_is_bounded() {
    let emulator = Emulator::new();

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
}
