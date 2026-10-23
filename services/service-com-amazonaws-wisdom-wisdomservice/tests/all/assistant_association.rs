// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Wisdom assistant-association slice
//! (`CreateAssistantAssociation` + `GetAssistantAssociation` +
//! `DeleteAssistantAssociation`), driven over HTTP through the same router the emulator
//! serves with.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * a create persists the association it mints, so a later read — by id or by ARN,
//!   under either form of the parent's label — answers out of the stored record, for
//!   every knowledge base and every tag count the create accepts;
//! * ONE assistant holds associations with several DISTINCT knowledge bases, while a
//!   second create naming a knowledge base it already holds is `ConflictException`
//!   (HTTP 409);
//! * a replayed `clientToken` carrying the same request answers with the association it
//!   already created, and the same token carrying a different request is
//!   `ConflictException` (HTTP 409);
//! * a label this service holds nothing under — an unknown assistant, an unknown
//!   knowledge base, an association of ANOTHER assistant — is
//!   `ResourceNotFoundException` (HTTP 404);
//! * the tag map is bounded, so a create past the ceiling is `ValidationException`
//!   (HTTP 400);
//! * a delete removes the record, so a read of it is not found and the same knowledge
//!   base can be associated again; deleting the assistant takes its associations with
//!   it.
//!
//! The knowledge bases the associations are made against are put in the store directly:
//! this crate's `CreateKnowledgeBase` is still a scaffold stub (its own issue), and the
//! association create resolves its `knowledgeBaseId` against the registry that create
//! will fill. Everything else goes over the wire, so a response that is not computed
//! from persisted state cannot pass.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for; the handlers read it out of
/// the SigV4 credential scope to build ARNs.
const REGION: &str = "us-west-2";

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`): the shared `wisdom` endpoint identity is served by the
/// Amazon Q in Connect crate, so this legacy model is addressed by its own key.
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// A UUID no resource of this suite is created under, for the not-found assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The only `AssociationType` this model declares
/// (`smithy_query(query="shape",shape="AssociationType")`), written out rather than
/// imported from the implementation so the assertions pin the modelled value.
const TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn field(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer).filter(|value| !value.is_null())
    }

    fn string(&self, pointer: &str) -> String {
        self.field(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router. The engine is kept so the suite can put the knowledge
/// bases an association is made against in the store.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{ROUTE_KEY}/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(format!("https://wisdom.{REGION}.amazonaws.com{uri}"))
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
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.body().clone();
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

    /// An assistant to hang associations off, returned as (id, ARN).
    async fn assistant(&self, name: &str) -> (String, String) {
        let reply = self
            .send(
                "POST",
                "/assistants",
                Some(json!({ "name": name, "type": "AGENT" })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"assistant {name}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        (
            reply.string("/assistant/assistantId"),
            reply.string("/assistant/assistantArn"),
        )
    }

    /// Put one knowledge base in the store under `knowledge_base_id`, carrying `arn`,
    /// and hand back that ARN: an association reports the ARN of the STORED knowledge
    /// base, so the test seeds the value it then asserts on.
    fn seed_knowledge_base(&self, knowledge_base_id: &str, arn: &str) -> String {
        let store = state::service_state(&*self.engine).expect("this service's store");
        store
            .lock()
            .insert_knowledge_base(state::KnowledgeBase {
                knowledge_base_id: knowledge_base_id.to_string(),
                knowledge_base_arn: arn.to_string(),
            });
        arn.to_string()
    }

    async fn create_association(&self, assistant_label: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{}/associations", label(assistant_label)),
            Some(body),
        )
        .await
    }

    async fn get_association(&self, assistant_label: &str, association_label: &str) -> Reply {
        self.send(
            "GET",
            &format!(
                "/assistants/{}/associations/{}",
                label(assistant_label),
                label(association_label)
            ),
            None,
        )
        .await
    }

    async fn delete_association(&self, assistant_label: &str, association_label: &str) -> Reply {
        self.send(
            "DELETE",
            &format!(
                "/assistants/{}/associations/{}",
                label(assistant_label),
                label(association_label)
            ),
            None,
        )
        .await
    }

    async fn delete_assistant(&self, assistant_label: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/assistants/{}", label(assistant_label)),
            None,
        )
        .await
    }
}

/// Percent-encode one path label the way an AWS SDK encodes a non-greedy `@httpLabel`:
/// an ARN label carries `:` and `/`, which must not be read as path structure. A bare
/// uuid label passes through unchanged.
fn label(value: &str) -> String {
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

/// The assistant-association ARN AWS documents for this service:
/// `arn:aws:wisdom:<region>:<account>:association/<assistantId>/<assistantAssociationId>`.
/// Written out here rather than taken from the handler's helper, so the test pins the
/// format.
fn expected_association_arn(assistant_id: &str, association_id: &str) -> String {
    format!(
        "arn:aws:wisdom:{REGION}:{account}:association/{assistant_id}/{association_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// A knowledge-base ARN of this account, for a knowledge base the suite seeds.
fn knowledge_base_arn(knowledge_base_id: &str) -> String {
    format!(
        "arn:aws:wisdom:{REGION}:{account}:knowledge-base/{knowledge_base_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// Whether `value` is a lowercase hyphenated uuid, the form the model's `Uuid` pattern
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

/// `count` tags, so a row can vary the tag count without writing each map out.
fn tag_map(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect()
}

/// One row of the create/read table.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    knowledge_base_id: &'static str,
    tags: BTreeMap<String, String>,
    client_token: Option<&'static str>,
}

/// The table: one row per axis the create rule generalises over — the knowledge base
/// associated (a distinct one per row, all under ONE assistant), the tag count (none,
/// one, several, and the published ceiling) and whether the caller supplied a
/// `clientToken`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one tag, with a clientToken",
            knowledge_base_id: "11111111-1111-1111-1111-111111111111",
            tags: tag_map(1),
            client_token: Some("tok-one-tag"),
        },
        Case {
            label: "no tags, no clientToken",
            knowledge_base_id: "22222222-2222-2222-2222-222222222222",
            tags: tag_map(0),
            client_token: None,
        },
        Case {
            label: "three tags",
            knowledge_base_id: "33333333-3333-3333-3333-333333333333",
            tags: tag_map(3),
            client_token: None,
        },
        Case {
            // The ceiling itself: `state::MAX_TAGS_PER_RESOURCE` tags are accepted, one
            // more is refused (see the refusal test below).
            label: "the maximum number of tags",
            knowledge_base_id: "44444444-4444-4444-4444-444444444444",
            tags: tag_map(state::MAX_TAGS_PER_RESOURCE),
            client_token: Some("tok-max-tags"),
        },
    ]
}

/// A create body for one row.
fn create_body(case: &Case) -> Value {
    let mut body = json!({
        "associationType": TYPE_KNOWLEDGE_BASE,
        "association": { "knowledgeBaseId": case.knowledge_base_id },
    });
    if !case.tags.is_empty() {
        body["tags"] = json!(case.tags);
    }
    if let Some(token) = case.client_token {
        body["clientToken"] = json!(token);
    }
    body
}

/// A create persists what it mints: every row of the table is read back — by id and by
/// ARN, under either form of the parent's label — out of the stored record, and ONE
/// assistant holds every one of them, so several distinct knowledge bases on one
/// assistant is a success and not a conflict.
#[tokio::test]
async fn a_create_persists_the_association_and_every_read_answers_from_the_record() {
    let emulator = Emulator::new();
    let (assistant_id, assistant_arn) = emulator.assistant("assoc-table-assistant").await;

    for case in cases() {
        let knowledge_base_arn = emulator.seed_knowledge_base(
            case.knowledge_base_id,
            &knowledge_base_arn(case.knowledge_base_id),
        );
        let created = emulator
            .create_association(&assistant_id, create_body(&case))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200, got {} with {}",
            case.label, created.status, created.body
        );

        let association_id = created.string("/assistantAssociation/assistantAssociationId");
        assert!(
            is_modelled_uuid(&association_id),
            "case {:?}: expected a modelled Uuid assistantAssociationId, got {association_id:?}",
            case.label
        );
        let expected = json!({
            "assistantAssociationId": association_id,
            "assistantAssociationArn": expected_association_arn(&assistant_id, &association_id),
            "assistantId": assistant_id,
            "assistantArn": assistant_arn,
            "associationType": TYPE_KNOWLEDGE_BASE,
            "associationData": {
                "knowledgeBaseAssociation": {
                    "knowledgeBaseId": case.knowledge_base_id,
                    "knowledgeBaseArn": knowledge_base_arn,
                }
            },
            "tags": json!(case.tags),
        });
        let actual = created
            .field("/assistantAssociation")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            actual, expected,
            "case {:?}: the create must answer out of the record it stored, expected \
             {expected} got {actual}",
            case.label
        );

        for (read_label, association_label) in [
            ("association id", association_id.clone()),
            (
                "association ARN",
                expected_association_arn(&assistant_id, &association_id),
            ),
        ] {
            for (parent_label, parent) in [
                ("assistant id", assistant_id.clone()),
                ("assistant ARN", assistant_arn.clone()),
            ] {
                let read = emulator.get_association(&parent, &association_label).await;
                assert_eq!(
                    read.status, 200,
                    "case {:?} read by {read_label} under {parent_label}: expected HTTP 200, \
                     got {} with {}",
                    case.label, read.status, read.body
                );
                let read_actual = read
                    .field("/assistantAssociation")
                    .cloned()
                    .unwrap_or(Value::Null);
                assert_eq!(
                    read_actual, expected,
                    "case {:?} read by {read_label} under {parent_label}: a read must answer \
                     what the create answered, expected {expected} got {read_actual}",
                    case.label
                );
            }
        }
    }
}

/// A second create naming a knowledge base the assistant is already associated with is
/// the modelled conflict, and the refusal leaves the first association readable.
#[tokio::test]
async fn a_knowledge_base_the_assistant_already_holds_is_a_conflict() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-conflict-assistant").await;

    for case in cases() {
        emulator.seed_knowledge_base(
            case.knowledge_base_id,
            &knowledge_base_arn(case.knowledge_base_id),
        );
        // The same request twice with NO clientToken: the second call is not a retry of
        // the first, it is a second create of an association that already exists.
        let mut body = create_body(&case);
        body.as_object_mut()
            .expect("a JSON object body")
            .remove("clientToken");

        let first = emulator
            .create_association(&assistant_id, body.clone())
            .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: expected the first create to answer HTTP 200, got {} with {}",
            case.label, first.status, first.body
        );
        let association_id = first.string("/assistantAssociation/assistantAssociationId");

        let second = emulator.create_association(&assistant_id, body).await;
        assert_eq!(
            second.status, 409,
            "case {:?}: expected HTTP 409 for a knowledge base already associated, got {} \
             with {}",
            case.label, second.status, second.body
        );
        assert!(
            second.error_type.starts_with("ConflictException"),
            "case {:?}: expected ConflictException, got {:?} with {}",
            case.label,
            second.error_type,
            second.body
        );

        let read = emulator
            .get_association(&assistant_id, &association_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the refused create must leave the first association readable, got {} \
             with {}",
            case.label, read.status, read.body
        );
    }
}

/// An idempotency token identifies ONE request: a replay carrying the identical request
/// answers with the association that token already created, and the SAME token carrying
/// any changed parameter is a conflict rather than a second resource or a silently
/// replayed first one.
#[tokio::test]
async fn a_client_token_replay_is_idempotent_and_a_changed_request_conflicts() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-token-assistant").await;
    let first_kb = "55555555-5555-5555-5555-555555555555";
    let other_kb = "66666666-6666-6666-6666-666666666666";
    emulator.seed_knowledge_base(first_kb, &knowledge_base_arn(first_kb));
    emulator.seed_knowledge_base(other_kb, &knowledge_base_arn(other_kb));
    let token = "tok-idempotent";

    let first = emulator
        .create_association(
            &assistant_id,
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": first_kb },
                "clientToken": token,
                "tags": { "env": "test", "team": "support" },
            }),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "case \"first create\": expected HTTP 200, got {} with {}",
        first.status, first.body
    );
    let association_id = first.string("/assistantAssociation/assistantAssociationId");

    // The identical request, with the tag map written in another order: one request, so
    // one resource.
    let replay = emulator
        .create_association(
            &assistant_id,
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": first_kb },
                "clientToken": token,
                "tags": { "team": "support", "env": "test" },
            }),
        )
        .await;
    assert_eq!(
        replay.status, 200,
        "case \"identical replay\": expected HTTP 200, got {} with {}",
        replay.status, replay.body
    );
    let replayed_id = replay.string("/assistantAssociation/assistantAssociationId");
    assert_eq!(
        replayed_id, association_id,
        "case \"identical replay\": expected the association {association_id} the token \
         already created, got {replayed_id}"
    );

    let changed = [
        (
            "a different knowledge base",
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": other_kb },
                "clientToken": token,
                "tags": { "env": "test", "team": "support" },
            }),
        ),
        (
            "different tag values",
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": first_kb },
                "clientToken": token,
                "tags": { "env": "prod", "team": "support" },
            }),
        ),
        (
            "one tag fewer",
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": first_kb },
                "clientToken": token,
                "tags": { "env": "test" },
            }),
        ),
        (
            "no tags at all",
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": first_kb },
                "clientToken": token,
            }),
        ),
    ];
    for (case, body) in changed {
        let reply = emulator.create_association(&assistant_id, body).await;
        assert_eq!(
            reply.status, 409,
            "case {case:?}: expected HTTP 409 for a token reused with a changed request, got \
             {} with {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.starts_with("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?} with {}",
            reply.error_type,
            reply.body
        );
    }

    // The same token under ANOTHER assistant is a different create, not a replay.
    let (other_assistant_id, _) = emulator.assistant("assoc-token-other-assistant").await;
    let elsewhere = emulator
        .create_association(
            &other_assistant_id,
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": first_kb },
                "clientToken": token,
                "tags": { "env": "test", "team": "support" },
            }),
        )
        .await;
    assert_eq!(
        elsewhere.status, 200,
        "case \"the same token under another assistant\": expected HTTP 200, got {} with {}",
        elsewhere.status, elsewhere.body
    );
    let elsewhere_id = elsewhere.string("/assistantAssociation/assistantAssociationId");
    assert_ne!(
        elsewhere_id, association_id,
        "case \"the same token under another assistant\": expected a different association, \
         got the first assistant's {association_id}"
    );
}

/// A label this service holds nothing under is the modelled not-found, for every way a
/// request can name something absent.
#[tokio::test]
async fn labels_that_name_nothing_are_not_found() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-404-assistant").await;
    let (other_assistant_id, _) = emulator.assistant("assoc-404-other-assistant").await;
    let knowledge_base_id = "77777777-7777-7777-7777-777777777777";
    emulator.seed_knowledge_base(knowledge_base_id, &knowledge_base_arn(knowledge_base_id));
    let created = emulator
        .create_association(
            &assistant_id,
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": knowledge_base_id },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"setup create\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
    let association_id = created.string("/assistantAssociation/assistantAssociationId");

    let create_rows = [
        (
            "an assistant this account does not hold",
            ABSENT_UUID.to_string(),
            knowledge_base_id.to_string(),
        ),
        (
            "an assistant ARN this account holds nothing under",
            format!(
                "arn:aws:wisdom:{REGION}:{account}:assistant/{ABSENT_UUID}",
                account = engine::EMULATOR_ACCOUNT_ID
            ),
            knowledge_base_id.to_string(),
        ),
        (
            "a knowledge base this account does not hold",
            assistant_id.clone(),
            ABSENT_UUID.to_string(),
        ),
    ];
    for (case, parent, knowledge_base) in create_rows {
        let reply = emulator
            .create_association(
                &parent,
                json!({
                    "associationType": TYPE_KNOWLEDGE_BASE,
                    "association": { "knowledgeBaseId": knowledge_base },
                }),
            )
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.starts_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} with {}",
            reply.error_type,
            reply.body
        );
    }

    let read_rows = [
        (
            "an assistant this account does not hold",
            ABSENT_UUID.to_string(),
            association_id.clone(),
        ),
        (
            "an association id nothing was created under",
            assistant_id.clone(),
            ABSENT_UUID.to_string(),
        ),
        (
            "an association of another assistant",
            other_assistant_id.clone(),
            association_id.clone(),
        ),
        (
            "an association ARN of another account",
            assistant_id.clone(),
            format!(
                "arn:aws:wisdom:{REGION}:999999999999:association/{assistant_id}/{association_id}"
            ),
        ),
    ];
    for (case, parent, association) in read_rows {
        for method in ["GET", "DELETE"] {
            let reply = match method {
                "GET" => emulator.get_association(&parent, &association).await,
                _ => emulator.delete_association(&parent, &association).await,
            };
            assert_eq!(
                reply.status, 404,
                "case {case:?} on {method}: expected HTTP 404, got {} with {}",
                reply.status, reply.body
            );
            assert!(
                reply.error_type.starts_with("ResourceNotFoundException"),
                "case {case:?} on {method}: expected ResourceNotFoundException, got {:?} with \
                 {}",
                reply.error_type,
                reply.body
            );
        }
    }
}

/// The tag map is bounded: a create carrying one tag past the ceiling is refused with
/// the modelled `ValidationException`, and nothing is stored for it.
#[tokio::test]
async fn a_tag_map_past_the_ceiling_is_refused() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-tags-assistant").await;
    let knowledge_base_id = "88888888-8888-8888-8888-888888888888";
    emulator.seed_knowledge_base(knowledge_base_id, &knowledge_base_arn(knowledge_base_id));

    for over in [1usize, 5] {
        let count = state::MAX_TAGS_PER_RESOURCE + over;
        let reply = emulator
            .create_association(
                &assistant_id,
                json!({
                    "associationType": TYPE_KNOWLEDGE_BASE,
                    "association": { "knowledgeBaseId": knowledge_base_id },
                    "tags": json!(tag_map(count)),
                }),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case \"{count} tags\": expected HTTP 400, got {} with {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.starts_with("ValidationException"),
            "case \"{count} tags\": expected ValidationException, got {:?} with {}",
            reply.error_type,
            reply.body
        );
    }

    // The refusals stored nothing, so the knowledge base is still free to associate.
    let accepted = emulator
        .create_association(
            &assistant_id,
            json!({
                "associationType": TYPE_KNOWLEDGE_BASE,
                "association": { "knowledgeBaseId": knowledge_base_id },
                "tags": json!(tag_map(state::MAX_TAGS_PER_RESOURCE)),
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"the ceiling itself\": expected HTTP 200, got {} with {}",
        accepted.status, accepted.body
    );
}

/// A delete removes the record: the association is no longer readable, the same
/// knowledge base can be associated again, and deleting the assistant takes its
/// associations with it.
#[tokio::test]
async fn a_delete_removes_the_association_and_frees_its_knowledge_base() {
    let emulator = Emulator::new();
    let (assistant_id, _) = emulator.assistant("assoc-delete-assistant").await;
    let knowledge_base_id = "99999999-9999-9999-9999-999999999999";
    emulator.seed_knowledge_base(knowledge_base_id, &knowledge_base_arn(knowledge_base_id));
    let body = json!({
        "associationType": TYPE_KNOWLEDGE_BASE,
        "association": { "knowledgeBaseId": knowledge_base_id },
        "clientToken": "tok-deleted",
    });

    let created = emulator
        .create_association(&assistant_id, body.clone())
        .await;
    assert_eq!(
        created.status, 200,
        "case \"create before delete\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
    let association_id = created.string("/assistantAssociation/assistantAssociationId");

    let deleted = emulator
        .delete_association(&assistant_id, &association_id)
        .await;
    assert_eq!(
        deleted.status, 204,
        "case \"delete\": expected HTTP 204, got {} with {}",
        deleted.status, deleted.body
    );

    let read = emulator
        .get_association(&assistant_id, &association_id)
        .await;
    assert_eq!(
        read.status, 404,
        "case \"read after delete\": expected HTTP 404, got {} with {}",
        read.status, read.body
    );

    // The knowledge base is free again, and the create that takes it — carrying the
    // very token the deleted association was made with — mints a NEW association rather
    // than resurrecting the deleted one.
    let recreated = emulator.create_association(&assistant_id, body).await;
    assert_eq!(
        recreated.status, 200,
        "case \"re-create after delete\": expected HTTP 200, got {} with {}",
        recreated.status, recreated.body
    );
    let recreated_id = recreated.string("/assistantAssociation/assistantAssociationId");
    assert_ne!(
        recreated_id, association_id,
        "case \"re-create after delete\": expected a new assistantAssociationId, got the \
         deleted one {association_id}"
    );

    // An association lives inside its assistant, so nothing of it survives the parent.
    let gone = emulator.delete_assistant(&assistant_id).await;
    assert_eq!(
        gone.status, 204,
        "case \"delete the assistant\": expected HTTP 204, got {} with {}",
        gone.status, gone.body
    );
    let orphan = emulator
        .get_association(&assistant_id, &recreated_id)
        .await;
    assert_eq!(
        orphan.status, 404,
        "case \"read after the assistant is deleted\": expected HTTP 404, got {} with {}",
        orphan.status, orphan.body
    );
}

/// Two assistants can be associated with the SAME knowledge base: the
/// one-association-per-knowledge-base rule is per assistant, not per knowledge base.
#[tokio::test]
async fn one_knowledge_base_can_be_associated_with_several_assistants() {
    let emulator = Emulator::new();
    let knowledge_base_id = "abababab-abab-abab-abab-abababababab";
    let arn = emulator.seed_knowledge_base(knowledge_base_id, &knowledge_base_arn(knowledge_base_id));
    let mut seen: Vec<String> = Vec::new();

    for name in ["shared-assistant-a", "shared-assistant-b", "shared-assistant-c"] {
        let (assistant_id, _) = emulator.assistant(name).await;
        let created = emulator
            .create_association(
                &assistant_id,
                json!({
                    "associationType": TYPE_KNOWLEDGE_BASE,
                    "association": { "knowledgeBaseId": knowledge_base_id },
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200, got {} with {}",
            created.status, created.body
        );
        let reported = created.string(
            "/assistantAssociation/associationData/knowledgeBaseAssociation/knowledgeBaseArn",
        );
        assert_eq!(
            reported, arn,
            "case {name:?}: expected the stored knowledge base's ARN {arn}, got {reported}"
        );
        let association_id = created.string("/assistantAssociation/assistantAssociationId");
        assert!(
            !seen.contains(&association_id),
            "case {name:?}: expected a fresh assistantAssociationId, got the already seen \
             {association_id}"
        );
        seen.push(association_id);
    }
}
