// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListTagsForResource`, driven over HTTP through the same router
//! the emulator serves with.
//!
//! The rule under test, asserted over a TABLE of inputs rather than the one recorded
//! request: `ListTagsForResource` REPORTS the tag map persisted on the resource its
//! `resourceArn` names, for EVERY resource family this service holds tags on, and an ARN
//! that names no resource this account holds is the modelled `ResourceNotFoundException`
//! (HTTP 404).
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * the resource family the ARN belongs to — an assistant, a knowledge base, a session
//!   and an assistant association, which are the four families this model's implemented
//!   surface creates;
//! * the number of tags (none, one, several, and the store's ceiling), their keys' and
//!   values' forms, and both published length bounds of `TagKey` and `TagValue`;
//! * for a not-found row: the family label, the id, the region, the account and the
//!   partition the ARN names, and whether the resource it named was deleted.
//!
//! What is asserted is that the answer is COMPUTED from what a previous call persisted:
//! the reported map equals the map that create stored (and the `tags` member that create
//! itself answered with), one resource's tags are never another's, a second read of an
//! unchanged resource answers alike, and an ARN whose record is gone is not found.
//! Every assertion goes over the wire, so a response that is not computed from persisted
//! state cannot pass.
//!
//! This view of the shared `wisdom` endpoint answers under its own registration key (see
//! `src/service.rs`), which is what every request below is signed for.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for; the handlers read it out of the
/// SigV4 credential scope to build ARNs.
const REGION: &str = "eu-west-1";

/// A region no resource of this suite is created in, for the not-found rows.
const OTHER_REGION: &str = "ap-northeast-1";

/// An account id no resource of this suite is created under, for the not-found rows.
/// Twelve digits, which is what the modelled `Arn` pattern admits (`[0-9]{12}`).
const OTHER_ACCOUNT: &str = "999999999999";

/// The routing key this service answers under (`src/service.rs`'s
/// `OWN_REGISTRATION_KEY`): the shared `wisdom` endpoint identity is served by the
/// Amazon Q in Connect crate, so this legacy model is addressed by its own key.
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// A UUID no resource of this suite is created under, for the not-found rows.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The only `AssociationType` this model declares
/// (`smithy_query(query="shape",shape="AssociationType")`).
const TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";

/// The published bounds of the modelled tag map's key and value
/// (`smithy_query(query="shape",shape="Tags")`: `TagKey` len 1..128, `TagValue` len
/// 1..256).
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MIN: usize = 1;
const TAG_VALUE_MAX: usize = 256;

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

    /// `ListTagsForResource` on one resource ARN.
    async fn list_tags(&self, resource_arn: &str) -> Reply {
        self.send("GET", &format!("/tags/{}", label(resource_arn)), None)
            .await
    }

    async fn create_assistant(&self, name: &str, tags: &BTreeMap<String, String>) -> Reply {
        self.send(
            "POST",
            "/assistants",
            Some(with_tags(json!({ "name": name, "type": "AGENT" }), tags)),
        )
        .await
    }

    async fn create_knowledge_base(&self, name: &str, tags: &BTreeMap<String, String>) -> Reply {
        self.send(
            "POST",
            "/knowledgeBases",
            Some(with_tags(
                json!({ "name": name, "knowledgeBaseType": "CUSTOM" }),
                tags,
            )),
        )
        .await
    }

    async fn create_session(
        &self,
        assistant_id: &str,
        name: &str,
        tags: &BTreeMap<String, String>,
    ) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{}/sessions", label(assistant_id)),
            Some(with_tags(json!({ "name": name }), tags)),
        )
        .await
    }

    async fn create_association(
        &self,
        assistant_id: &str,
        knowledge_base_id: &str,
        tags: &BTreeMap<String, String>,
    ) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{}/associations", label(assistant_id)),
            Some(with_tags(
                json!({
                    "associationType": TYPE_KNOWLEDGE_BASE,
                    "association": { "knowledgeBaseId": knowledge_base_id },
                }),
                tags,
            )),
        )
        .await
    }

    async fn delete_assistant(&self, assistant_id: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/assistants/{}", label(assistant_id)),
            None,
        )
        .await
    }

    async fn delete_knowledge_base(&self, knowledge_base_id: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/knowledgeBases/{}", label(knowledge_base_id)),
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

/// `body` with the row's `tags` member added, and without it when the row carries no tag
/// — so an untagged resource is created exactly as a client that names no tags creates
/// one.
fn with_tags(mut body: Value, tags: &BTreeMap<String, String>) -> Value {
    if !tags.is_empty() {
        body["tags"] = json!(tags);
    }
    body
}

/// A wisdom ARN of this account, written out here rather than taken from the handler's
/// helper so the test pins the format the model's `Arn` pattern describes.
fn wisdom_arn(partition: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:{partition}:wisdom:{region}:{account}:{resource}")
}

/// The four resource families this model's implemented surface creates, which is what
/// the tagging surface's `resourceArn` can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Assistant,
    KnowledgeBase,
    Session,
    Association,
}

impl Family {
    /// The ARN resource label this family's ARN carries
    /// (`state::assistant_arn` and friends).
    fn arn_label(self) -> &'static str {
        match self {
            Family::Assistant => "assistant",
            Family::KnowledgeBase => "knowledge-base",
            Family::Session => "session",
            Family::Association => "association",
        }
    }

    /// The JSON pointer to the created resource's own ARN.
    fn arn_pointer(self) -> &'static str {
        match self {
            Family::Assistant => "/assistant/assistantArn",
            Family::KnowledgeBase => "/knowledgeBase/knowledgeBaseArn",
            Family::Session => "/session/sessionArn",
            Family::Association => "/assistantAssociation/assistantAssociationArn",
        }
    }

    /// The JSON pointer to the `tags` member the create itself reported.
    fn tags_pointer(self) -> &'static str {
        match self {
            Family::Assistant => "/assistant/tags",
            Family::KnowledgeBase => "/knowledgeBase/tags",
            Family::Session => "/session/tags",
            Family::Association => "/assistantAssociation/tags",
        }
    }
}

/// Every family, so the table spans the whole set rather than the one family the issue's
/// recording used.
const FAMILIES: [Family; 4] = [
    Family::Assistant,
    Family::KnowledgeBase,
    Family::Session,
    Family::Association,
];

/// `count` tags, so a row can vary the tag count without writing each map out.
fn tag_map(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect()
}

/// One row of the tag table: a tag map, and the name that says what it varies.
#[derive(Debug, Clone)]
struct TagCase {
    label: &'static str,
    tags: BTreeMap<String, String>,
}

/// The tag maps the table is run over: no tag at all, one, several, the store's ceiling,
/// every punctuation class the modelled `TagKey` pattern admits
/// (`^(?!aws:)[a-zA-Z+-=._:/]+$`), and both published length bounds of `TagKey` and
/// `TagValue`.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            label: "no tags at all",
            tags: BTreeMap::new(),
        },
        TagCase {
            label: "one tag",
            tags: tag_map(1),
        },
        TagCase {
            label: "three tags",
            tags: tag_map(3),
        },
        TagCase {
            label: "the punctuation the modelled TagKey pattern admits",
            tags: [
                ("Env", "Production"),
                ("a.b", "1"),
                ("team_1", "alpha"),
                ("path/sub", "nested"),
                ("plus+eq=", "signs"),
                ("ns:name", "colon"),
                ("dash-ed", "2"),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        },
        TagCase {
            label: "the published length bounds of TagKey and TagValue",
            tags: [
                ("k".repeat(TAG_KEY_MAX), "v".repeat(TAG_VALUE_MAX)),
                ("k".to_string(), "v".repeat(TAG_VALUE_MIN)),
            ]
            .into_iter()
            .collect(),
        },
        TagCase {
            label: "the store's tag ceiling",
            tags: tag_map(state::MAX_TAGS_PER_RESOURCE),
        },
    ]
}

/// Create one resource of `family` carrying `tags`, and hand back its ARN together with
/// the `tags` member the create itself answered with.
///
/// `seq` keeps the names (and the knowledge base an association is made against) distinct
/// between rows, since a duplicate name is a conflict and one assistant cannot associate
/// one knowledge base twice.
async fn create_tagged(
    emulator: &Emulator,
    family: Family,
    seq: usize,
    case: &str,
    host_assistant: &str,
    tags: &BTreeMap<String, String>,
) -> (String, Value) {
    let created = match family {
        Family::Assistant => {
            emulator
                .create_assistant(&format!("tagged-assistant-{seq}"), tags)
                .await
        }
        Family::KnowledgeBase => {
            emulator
                .create_knowledge_base(&format!("tagged-kb-{seq}"), tags)
                .await
        }
        Family::Session => {
            emulator
                .create_session(host_assistant, &format!("tagged-session-{seq}"), tags)
                .await
        }
        Family::Association => {
            // An association is made against a knowledge base, and one assistant holds
            // at most one association per knowledge base, so each row gets its own —
            // created UNTAGGED, so the tags the row asserts on can only be the
            // association's.
            let knowledge_base = emulator
                .create_knowledge_base(&format!("assoc-host-kb-{seq}"), &BTreeMap::new())
                .await;
            assert_eq!(
                knowledge_base.status, 200,
                "case {case:?} ({family:?}): expected the host knowledge base to be \
                 created with HTTP 200, got {} with {}",
                knowledge_base.status, knowledge_base.body
            );
            let knowledge_base_id = knowledge_base.string("/knowledgeBase/knowledgeBaseId");
            emulator
                .create_association(host_assistant, &knowledge_base_id, tags)
                .await
        }
    };
    assert_eq!(
        created.status, 200,
        "case {case:?} ({family:?}): expected the create to answer HTTP 200, got {} with {}",
        created.status, created.body
    );
    let arn = created.string(family.arn_pointer());
    assert!(
        arn.starts_with(&format!(
            "arn:aws:wisdom:{REGION}:{account}:{resource}/",
            account = engine::EMULATOR_ACCOUNT_ID,
            resource = family.arn_label()
        )),
        "case {case:?} ({family:?}): expected the created resource's ARN to name this \
         account's {} in {REGION}, got {arn:?}",
        family.arn_label()
    );
    let reported = created
        .field(family.tags_pointer())
        .cloned()
        .unwrap_or(Value::Null);
    (arn, reported)
}

/// The tag map `ListTagsForResource` reports for `resource_arn` is the one the create
/// persisted — and the one the create itself answered with — for every family and every
/// tag map in the table. The member is always present, and an empty object for a
/// resource carrying no tag.
#[tokio::test]
async fn list_tags_reports_the_map_persisted_on_the_resource_the_arn_names() {
    let emulator = Emulator::new();
    let host = emulator
        .create_assistant("tags-host-assistant", &BTreeMap::new())
        .await;
    assert_eq!(
        host.status, 200,
        "case \"host assistant\": expected HTTP 200, got {} with {}",
        host.status, host.body
    );
    let host_assistant = host.string("/assistant/assistantId");

    for (seq, case) in tag_cases().into_iter().enumerate() {
        for family in FAMILIES {
            let seq = seq * FAMILIES.len() + family as usize;
            let (arn, reported_by_create) = create_tagged(
                &emulator,
                family,
                seq,
                case.label,
                &host_assistant,
                &case.tags,
            )
            .await;

            let listed = emulator.list_tags(&arn).await;
            assert_eq!(
                listed.status, 200,
                "case {:?} ({family:?}): expected ListTagsForResource to answer HTTP 200 \
                 for {arn}, got {} with {}",
                case.label, listed.status, listed.body
            );
            let expected = json!({ "tags": json!(case.tags) });
            assert_eq!(
                listed.body, expected,
                "case {:?} ({family:?}): expected ListTagsForResource to report the map \
                 the create persisted, {expected}, got {}",
                case.label, listed.body
            );
            // The two reporting paths read ONE stored map, so they cannot disagree.
            assert_eq!(
                listed.field("/tags").cloned().unwrap_or(Value::Null),
                reported_by_create,
                "case {:?} ({family:?}): ListTagsForResource reported {:?} while the \
                 create reported {reported_by_create:?}",
                case.label,
                listed.field("/tags")
            );

            // The operation is `@readonly`: a second read of an unchanged resource
            // answers exactly as the first did.
            let again = emulator.list_tags(&arn).await;
            assert_eq!(
                again.body, listed.body,
                "case {:?} ({family:?}): a read must not change what a later read sees, \
                 expected {} got {}",
                case.label, listed.body, again.body
            );
        }
    }
}

/// One resource's tags are never another's: resources created side by side with different
/// maps each report their own.
#[tokio::test]
async fn each_resource_reports_only_its_own_tags() {
    let emulator = Emulator::new();
    let host = emulator
        .create_assistant("per-resource-host", &BTreeMap::new())
        .await;
    let host_assistant = host.string("/assistant/assistantId");

    let mut expected: Vec<(Family, &'static str, String, BTreeMap<String, String>)> = Vec::new();
    for (seq, family) in FAMILIES.into_iter().enumerate() {
        for (offset, case) in [
            ("first map", tag_map(1)),
            ("second map", tag_map(2)),
            ("no map", BTreeMap::new()),
        ]
        .into_iter()
        .enumerate()
        {
            let (name, tags) = case;
            let (arn, _) = create_tagged(
                &emulator,
                family,
                100 + seq * 10 + offset,
                name,
                &host_assistant,
                &tags,
            )
            .await;
            expected.push((family, name, arn, tags));
        }
    }

    for (family, name, arn, tags) in &expected {
        let listed = emulator.list_tags(arn).await;
        let want = json!({ "tags": json!(tags) });
        assert_eq!(
            listed.body, want,
            "case {name:?} ({family:?}): expected {arn} to report only its own tags, \
             {want}, got {}",
            listed.body
        );
    }
}

/// One row of the not-found table: an ARN that names no resource this account holds.
#[derive(Debug)]
struct MissCase {
    label: &'static str,
    arn: String,
}

/// An ARN this account holds no resource under is the modelled
/// `ResourceNotFoundException` (HTTP 404), whatever makes it miss: the family label, the
/// id, the region, the account or the partition.
#[tokio::test]
async fn an_arn_naming_no_resource_this_account_holds_is_not_found() {
    let emulator = Emulator::new();
    let assistant = emulator
        .create_assistant("miss-host-assistant", &tag_map(1))
        .await;
    let assistant_id = assistant.string("/assistant/assistantId");
    let assistant_arn = assistant.string("/assistant/assistantArn");
    let session = emulator
        .create_session(&assistant_id, "miss-host-session", &tag_map(1))
        .await;
    let session_id = session.string("/session/sessionId");
    let knowledge_base = emulator
        .create_knowledge_base("miss-host-kb", &tag_map(1))
        .await;
    let knowledge_base_id = knowledge_base.string("/knowledgeBase/knowledgeBaseId");

    let account = engine::EMULATOR_ACCOUNT_ID;
    let mut cases = vec![
        MissCase {
            label: "the right region and account, an id nothing holds",
            arn: wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("assistant/{ABSENT_UUID}"),
            ),
        },
        MissCase {
            label: "a stored assistant's id under another family's label",
            arn: wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("knowledge-base/{assistant_id}"),
            ),
        },
        MissCase {
            label: "a stored knowledge base's id under another family's label",
            arn: wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("assistant/{knowledge_base_id}"),
            ),
        },
        MissCase {
            label: "a stored assistant in another region",
            arn: wisdom_arn(
                "aws",
                OTHER_REGION,
                account,
                &format!("assistant/{assistant_id}"),
            ),
        },
        MissCase {
            label: "a stored assistant under another account",
            arn: wisdom_arn(
                "aws",
                REGION,
                OTHER_ACCOUNT,
                &format!("assistant/{assistant_id}"),
            ),
        },
        MissCase {
            label: "a stored assistant in another partition",
            arn: wisdom_arn(
                "aws-cn",
                REGION,
                account,
                &format!("assistant/{assistant_id}"),
            ),
        },
        MissCase {
            label: "a stored session under another assistant as its parent",
            arn: wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("session/{ABSENT_UUID}/{session_id}"),
            ),
        },
        MissCase {
            label: "a nested id nothing holds under a stored assistant",
            arn: wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("association/{assistant_id}/{ABSENT_UUID}"),
            ),
        },
    ];

    // A resource a DELETE removed holds no tags any more: its ARN, which used to name
    // it, names nothing.
    assert_eq!(
        emulator
            .delete_knowledge_base(&knowledge_base_id)
            .await
            .status,
        204,
        "case \"delete the host knowledge base\": expected the modelled 204"
    );
    cases.push(MissCase {
        label: "a knowledge base a delete removed",
        arn: knowledge_base.string("/knowledgeBase/knowledgeBaseArn"),
    });
    // Deleting the assistant takes its sessions with it, so both ARNs stop naming
    // anything.
    assert_eq!(
        emulator.delete_assistant(&assistant_id).await.status,
        204,
        "case \"delete the host assistant\": expected the modelled 204"
    );
    cases.push(MissCase {
        label: "an assistant a delete removed",
        arn: assistant_arn,
    });
    cases.push(MissCase {
        label: "a session whose assistant a delete removed",
        arn: session.string("/session/sessionArn"),
    });

    for case in &cases {
        let reply = emulator.list_tags(&case.arn).await;
        assert_eq!(
            reply.status, 404,
            "case {:?}: expected HTTP 404 for {}, got {} with {}",
            case.label, case.arn, reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ResourceNotFoundException"),
            "case {:?}: expected the modelled ResourceNotFoundException for {}, got \
             x-amzn-errortype {:?}",
            case.label,
            case.arn,
            reply.error_type
        );
        assert_eq!(
            reply.string("/resourceName"),
            case.arn,
            "case {:?}: expected the refusal to name the ARN asked about in \
             resourceName, got {:?}",
            case.label,
            reply.string("/resourceName")
        );
    }
}

/// A `resourceArn` outside the modelled `Arn` pattern is refused as a bad request
/// (HTTP 400), not as a resource that happens to be missing: the pattern is a modelled
/// constraint, so the refusal happens before the lookup.
#[tokio::test]
async fn a_resource_arn_outside_the_modelled_pattern_is_refused() {
    let emulator = Emulator::new();
    let account = engine::EMULATOR_ACCOUNT_ID;
    let cases = [
        ("a bare uuid, not an ARN", ABSENT_UUID.to_string()),
        (
            "an ARN of another service",
            format!("arn:aws:sns:{REGION}:{account}:topic/{ABSENT_UUID}"),
        ),
        (
            "a wisdom ARN whose resource id is no uuid",
            wisdom_arn("aws", REGION, account, "assistant/not-a-uuid"),
        ),
        (
            "a wisdom ARN with no resource id at all",
            wisdom_arn("aws", REGION, account, "assistant"),
        ),
    ];

    for (label, arn) in &cases {
        let reply = emulator.list_tags(arn).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400 for {arn}, got {} with {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ValidationException"),
            "case {label:?}: expected the modelled ValidationException for {arn}, got \
             x-amzn-errortype {:?}",
            reply.error_type
        );
    }
}
