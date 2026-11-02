// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Connect Wisdom quick-response slice
//! (`CreateQuickResponse` + `GetQuickResponse`).
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
//! * a create of a resource nested in a knowledge base persists every member the request
//!   carried and answers out of the stored record, so a later read — by id or by ARN —
//!   reports the same values, for every media type, every documented language, both
//!   published bounds of `name` and `content`, and each optional member the create
//!   accepts;
//! * the `name` and the `shortcutKey` of a quick response are unique INSIDE one
//!   knowledge base: a second create claiming either is `ConflictException` (HTTP 409)
//!   and leaves the first record untouched, while another knowledge base may hold both;
//! * a replayed `clientToken` answers with the quick response that token already created
//!   rather than minting a second one, scoped to the knowledge base it created under;
//! * a label this knowledge base holds no quick response under — a bare id, an ARN of
//!   this account, of another account or of another partition, or another knowledge
//!   base's quick response — is `ResourceNotFoundException` (HTTP 404), as is a create
//!   under a knowledge base this account does not hold;
//! * deleting a knowledge base forgets the quick responses held inside it;
//! * the tags a create persisted on a quick response are what `ListTagsForResource`
//!   reports for its ARN;
//! * one knowledge base holds at most the store's quick-response ceiling, past which a
//!   create is the modelled `ServiceQuotaExceededException` (HTTP 402).
//!
//! This view of the shared `wisdom` endpoint answers under its own registration key (see
//! `src/service.rs`), which is what every request below is signed for.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::state::MAX_QUICK_RESPONSES_PER_KNOWLEDGE_BASE;
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

/// The `QuickResponseStatus` a quick response this service holds reports. Written out as
/// the literal rather than imported from the implementation, so the assertion pins the
/// modelled value (`smithy_query(query="shape",shape="QuickResponseStatus")` declares it
/// beside the transient `CREATE_IN_PROGRESS`, `UPDATE_IN_PROGRESS` and the rest).
const STATUS_CREATED: &str = "CREATED";

/// Both values the modelled `QuickResponseType` pattern admits
/// (`smithy_query(query="operation",operation="CreateQuickResponse")`: `contentType`
/// matches `^(application/x\.quickresponse;format=(plain|markdown))$`).
const CONTENT_TYPE_PLAIN: &str = "application/x.quickresponse;format=plain";
const CONTENT_TYPE_MARKDOWN: &str = "application/x.quickresponse;format=markdown";

/// Every language code the model documents for `language`
/// (`smithy_query(query="doc",shape="CreateQuickResponseRequest",member="language")`:
/// "The supported language codes include de_DE, en_US, es_ES, fr_FR, id_ID, it_IT,
/// ja_JP, ko_KR, pt_BR, zh_CN, zh_TW"), so the table spans the whole catalog rather than
/// the one code a single recording used.
const LANGUAGES: [&str; 11] = [
    "de_DE", "en_US", "es_ES", "fr_FR", "id_ID", "it_IT", "ja_JP", "ko_KR", "pt_BR", "zh_CN",
    "zh_TW",
];

/// The published bounds of the members this suite varies in length
/// (`smithy_query(query="operation",operation="CreateQuickResponse")`: `name` is `len
/// 1..40`, `description` `len 1..255`, `shortcutKey` `len 1..10`, and the content union's
/// member `len 1..1024`).
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 40;
const DESCRIPTION_MAX: usize = 255;
const SHORTCUT_KEY_MAX: usize = 10;
const CONTENT_MAX: usize = 1024;

/// The fixed sentences this endpoint answers a resource it does not hold with, written
/// out here rather than imported from the implementation: the one naming the quick
/// response, and the one naming its parent knowledge base. Each travels with the label
/// the request used in `ResourceNotFoundException`'s own `resourceName` member.
const MESSAGE_QUICK_RESPONSE_NOT_FOUND: &str = "QuickResponse does not exist.";
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";

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

    /// A knowledge base to hold quick responses, created the way every other suite does.
    async fn create_knowledge_base(&self, name: &str) -> Reply {
        self.send(
            "POST",
            "/knowledgeBases",
            Some(json!({ "name": name, "knowledgeBaseType": "QUICK_RESPONSES" })),
        )
        .await
    }

    async fn delete_knowledge_base(&self, label: &str) -> Reply {
        self.send("DELETE", &format!("/knowledgeBases/{}", path(label)), None)
            .await
    }

    async fn create_quick_response(&self, knowledge_base: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/knowledgeBases/{}/quickResponses", path(knowledge_base)),
            Some(body),
        )
        .await
    }

    async fn get_quick_response(&self, knowledge_base: &str, quick_response: &str) -> Reply {
        self.send(
            "GET",
            &format!(
                "/knowledgeBases/{}/quickResponses/{}",
                path(knowledge_base),
                path(quick_response)
            ),
            None,
        )
        .await
    }

    async fn list_quick_responses(
        &self,
        knowledge_base: &str,
        max_results: Option<usize>,
        next_token: Option<&str>,
    ) -> Reply {
        let mut uri = format!("/knowledgeBases/{}/quickResponses", path(knowledge_base));
        let mut query = Vec::new();
        if let Some(max_results) = max_results {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(next_token) = next_token {
            query.push(format!("nextToken={next_token}"));
        }
        if !query.is_empty() {
            uri.push('?');
            uri.push_str(&query.join("&"));
        }
        self.send("GET", &uri, None).await
    }

    async fn list_tags(&self, resource_arn: &str) -> Reply {
        self.send("GET", &format!("/tags/{}", path(resource_arn)), None)
            .await
    }

    /// A fresh knowledge base, answering with the id its create minted.
    async fn knowledge_base_id(&self, name: &str) -> String {
        let reply = self.create_knowledge_base(name).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: the knowledge base this suite nests under must be created, got \
             HTTP {} with body {}",
            reply.status, reply.body
        );
        reply
            .string("/knowledgeBase/knowledgeBaseId")
            .unwrap_or_else(|| panic!("case {name:?}: the create answered no knowledgeBaseId"))
    }
}

/// A `UuidOrArn` label as one URI path segment: an ARN carries `/`, which must not be
/// read as path structure.
fn path(label: &str) -> String {
    label.replace('/', "%2F")
}

/// The quick-response ARN AWS documents for this service:
/// `arn:aws:wisdom:<region>:<account>:quick-response/<knowledgeBaseId>/<quickResponseId>`.
/// Written out here rather than taken from the handler's helper, so the test pins the
/// format.
fn expected_quick_response_arn(
    region: &str,
    knowledge_base_id: &str,
    quick_response_id: &str,
) -> String {
    format!(
        "arn:aws:wisdom:{region}:{account}:quick-response/{knowledge_base_id}/{quick_response_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// Whether `value` is a lowercase hyphenated UUID, the form the model's `Uuid` shape
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

/// One row of the create/read table: what varies between quick responses.
#[derive(Debug, Clone)]
struct QuickResponseCase {
    label: &'static str,
    name: String,
    content: String,
    /// `None` exercises a create that names no `contentType` at all.
    content_type: Option<&'static str>,
    description: Option<String>,
    shortcut_key: Option<String>,
    is_active: Option<bool>,
    channels: Vec<String>,
    language: Option<&'static str>,
    grouping_criteria: Option<&'static str>,
    grouping_values: Vec<String>,
    tags: BTreeMap<String, String>,
}

impl QuickResponseCase {
    /// A row with every optional member absent, which the rows below vary from.
    fn minimal(label: &'static str, name: &str, content: &str) -> Self {
        Self {
            label,
            name: name.to_string(),
            content: content.to_string(),
            content_type: Some(CONTENT_TYPE_PLAIN),
            description: None,
            shortcut_key: None,
            is_active: None,
            channels: Vec::new(),
            language: None,
            grouping_criteria: None,
            grouping_values: Vec::new(),
            tags: BTreeMap::new(),
        }
    }

    fn create_body(&self) -> Value {
        let mut body = json!({
            "name": self.name,
            "content": { "content": self.content },
        });
        if let Some(content_type) = self.content_type {
            body["contentType"] = json!(content_type);
        }
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if let Some(shortcut_key) = &self.shortcut_key {
            body["shortcutKey"] = json!(shortcut_key);
        }
        if let Some(is_active) = self.is_active {
            body["isActive"] = json!(is_active);
        }
        if !self.channels.is_empty() {
            body["channels"] = json!(self.channels);
        }
        if let Some(language) = self.language {
            body["language"] = json!(language);
        }
        if self.grouping_criteria.is_some() || !self.grouping_values.is_empty() {
            let mut grouping = json!({});
            if let Some(criteria) = self.grouping_criteria {
                grouping["criteria"] = json!(criteria);
            }
            if !self.grouping_values.is_empty() {
                grouping["values"] = json!(self.grouping_values);
            }
            body["groupingConfiguration"] = grouping;
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        body
    }

    /// The media type the record is stored with: the one the request named, or the
    /// plain-text default for a request that named none.
    fn expected_content_type(&self) -> &str {
        self.content_type.unwrap_or(CONTENT_TYPE_PLAIN)
    }

    /// Which `QuickResponseContents` member carries the content back.
    fn expected_contents_pointer(&self) -> &'static str {
        if self.expected_content_type() == CONTENT_TYPE_MARKDOWN {
            "/quickResponse/contents/markdown/content"
        } else {
            "/quickResponse/contents/plainText/content"
        }
    }
}

/// The table: one row per axis the rules generalise over — the media type (both modelled
/// values and a request naming none), the name's and the content's length (both published
/// bounds), the presence of each optional member, the channel count, the grouping
/// configuration, and the tag count — plus one row per documented language code.
fn quick_response_cases() -> Vec<QuickResponseCase> {
    let mut cases = vec![
        QuickResponseCase {
            description: Some("the plain-text reply an agent inserts first".to_string()),
            shortcut_key: Some("/hi".to_string()),
            is_active: Some(true),
            channels: vec!["Chat".to_string()],
            language: Some("en_US"),
            grouping_criteria: Some("RoutingProfileArn"),
            grouping_values: vec!["sales".to_string()],
            tags: [("env".to_string(), "test".to_string())]
                .into_iter()
                .collect(),
            ..QuickResponseCase::minimal(
                "plain text with every optional member",
                "greeting",
                "Hello, how can I help?",
            )
        },
        QuickResponseCase {
            content_type: Some(CONTENT_TYPE_MARKDOWN),
            ..QuickResponseCase::minimal(
                "markdown with the longest legal content",
                "rich-reply",
                &"m".repeat(CONTENT_MAX),
            )
        },
        QuickResponseCase {
            content_type: None,
            ..QuickResponseCase::minimal(
                "no contentType at all, shortest legal name and content",
                &"q".repeat(NAME_MIN),
                "c",
            )
        },
        QuickResponseCase {
            description: Some("d".repeat(DESCRIPTION_MAX)),
            shortcut_key: Some("k".repeat(SHORTCUT_KEY_MAX)),
            is_active: Some(false),
            ..QuickResponseCase::minimal(
                "longest legal name, description and shortcut key, inactive",
                &"n".repeat(NAME_MAX),
                "a long reply",
            )
        },
        QuickResponseCase {
            channels: vec!["Chat".to_string(), "Email".to_string()],
            grouping_criteria: Some("RoutingProfileArn"),
            grouping_values: vec![
                format!(
                    "arn:aws:connect:{REGION}:{}:instance/11111111-1111-1111-1111-111111111111",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "sales".to_string(),
            ],
            tags: [
                ("env".to_string(), "test".to_string()),
                ("team_1".to_string(), "alpha".to_string()),
                ("a.b".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
            ..QuickResponseCase::minimal(
                "several channels, grouping values and tags",
                "grouped",
                "Thanks for waiting.",
            )
        },
    ];
    // One row per documented language code, so the catalog is covered and not only the
    // codes the rows above happen to use.
    for (index, language) in LANGUAGES.into_iter().enumerate() {
        cases.push(QuickResponseCase {
            label: "one row per documented language code",
            language: Some(language),
            ..QuickResponseCase::minimal(
                "one row per documented language code",
                &format!("localized-{index}-{language}"),
                "Localized reply",
            )
        });
    }
    cases
}

/// Every member of the reported quick response, checked against the row that created it.
fn assert_reported_quick_response(
    case: &QuickResponseCase,
    reply: &Reply,
    knowledge_base_id: &str,
    quick_response_id: &str,
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
        reply.string("/quickResponse/quickResponseId").as_deref(),
        Some(quick_response_id),
        "case {label:?} ({source}): expected the stored quickResponseId, got {:?}",
        reply.field("/quickResponse/quickResponseId")
    );
    assert_eq!(
        reply.string("/quickResponse/quickResponseArn"),
        Some(expected_quick_response_arn(
            REGION,
            knowledge_base_id,
            quick_response_id
        )),
        "case {label:?} ({source}): expected the quick-response ARN of the signed region, got \
         {:?}",
        reply.field("/quickResponse/quickResponseArn")
    );
    assert_eq!(
        reply.string("/quickResponse/knowledgeBaseId").as_deref(),
        Some(knowledge_base_id),
        "case {label:?} ({source}): expected the parent knowledgeBaseId, got {:?}",
        reply.field("/quickResponse/knowledgeBaseId")
    );
    assert!(
        reply
            .string("/quickResponse/knowledgeBaseArn")
            .is_some_and(|arn| arn.ends_with(&format!("knowledge-base/{knowledge_base_id}"))),
        "case {label:?} ({source}): expected the parent's own ARN, got {:?}",
        reply.field("/quickResponse/knowledgeBaseArn")
    );
    assert_eq!(
        reply.string("/quickResponse/name"),
        Some(case.name.clone()),
        "case {label:?} ({source}): expected the requested name, got {:?}",
        reply.field("/quickResponse/name")
    );
    assert_eq!(
        reply.string("/quickResponse/contentType").as_deref(),
        Some(case.expected_content_type()),
        "case {label:?} ({source}): expected the stored contentType, got {:?}",
        reply.field("/quickResponse/contentType")
    );
    assert_eq!(
        reply.string("/quickResponse/status").as_deref(),
        Some(STATUS_CREATED),
        "case {label:?} ({source}): expected status {STATUS_CREATED}, got {:?}",
        reply.field("/quickResponse/status")
    );
    assert_eq!(
        reply.string(case.expected_contents_pointer()),
        Some(case.content.clone()),
        "case {label:?} ({source}): expected the content under {}, got body {}",
        case.expected_contents_pointer(),
        reply.body
    );
    // The content is reported under exactly ONE member of the union-valued contents.
    let other_pointer = if case.expected_contents_pointer().contains("markdown") {
        "/quickResponse/contents/plainText"
    } else {
        "/quickResponse/contents/markdown"
    };
    assert!(
        reply.field(other_pointer).is_none(),
        "case {label:?} ({source}): {other_pointer} must be absent for contentType {}, got {:?}",
        case.expected_content_type(),
        reply.field(other_pointer)
    );
    assert_eq!(
        reply.string("/quickResponse/description"),
        case.description.clone(),
        "case {label:?} ({source}): expected the requested description, got {:?}",
        reply.field("/quickResponse/description")
    );
    assert_eq!(
        reply.string("/quickResponse/shortcutKey"),
        case.shortcut_key.clone(),
        "case {label:?} ({source}): expected the requested shortcutKey, got {:?}",
        reply.field("/quickResponse/shortcutKey")
    );
    assert_eq!(
        reply.field("/quickResponse/isActive").and_then(Value::as_bool),
        case.is_active,
        "case {label:?} ({source}): expected the requested isActive, got {:?}",
        reply.field("/quickResponse/isActive")
    );
    let reported_channels: Vec<String> = reply
        .field("/quickResponse/channels")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported_channels, case.channels,
        "case {label:?} ({source}): expected the requested channels, got {:?}",
        reply.field("/quickResponse/channels")
    );
    assert_eq!(
        reply.string("/quickResponse/language").as_deref(),
        case.language,
        "case {label:?} ({source}): expected the requested language, got {:?}",
        reply.field("/quickResponse/language")
    );
    assert_eq!(
        reply
            .string("/quickResponse/groupingConfiguration/criteria")
            .as_deref(),
        case.grouping_criteria,
        "case {label:?} ({source}): expected the requested grouping criteria, got {:?}",
        reply.field("/quickResponse/groupingConfiguration")
    );
    let reported_grouping_values: Vec<String> = reply
        .field("/quickResponse/groupingConfiguration/values")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported_grouping_values, case.grouping_values,
        "case {label:?} ({source}): expected the requested grouping values, got {:?}",
        reply.field("/quickResponse/groupingConfiguration")
    );
    let reported_tags: BTreeMap<String, String> = reply
        .field("/quickResponse/tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported_tags, case.tags,
        "case {label:?} ({source}): expected the requested tags, got {:?}",
        reply.field("/quickResponse/tags")
    );
    // The timestamps are epoch-seconds numbers of this run, and a record nothing has
    // modified was last written when it was created.
    let created = reply
        .field("/quickResponse/createdTime")
        .and_then(Value::as_f64);
    let modified = reply
        .field("/quickResponse/lastModifiedTime")
        .and_then(Value::as_f64);
    assert!(
        created.is_some_and(|seconds| seconds > 0.0),
        "case {label:?} ({source}): expected a createdTime in epoch seconds, got {:?}",
        reply.field("/quickResponse/createdTime")
    );
    assert_eq!(
        modified, created,
        "case {label:?} ({source}): an unmodified record reports lastModifiedTime equal to \
         createdTime, got {modified:?} and {created:?}"
    );
}

#[tokio::test]
async fn create_persists_every_member_and_a_read_answers_out_of_the_record() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base_id("quick-response-round-trip").await;
    let knowledge_base_arn = format!(
        "arn:aws:wisdom:{REGION}:{}:knowledge-base/{knowledge_base_id}",
        engine::EMULATOR_ACCOUNT_ID
    );

    for case in quick_response_cases() {
        let label = case.label;
        let created = emulator
            .create_quick_response(&knowledge_base_id, case.create_body())
            .await;
        let quick_response_id = created
            .string("/quickResponse/quickResponseId")
            .unwrap_or_else(|| {
                panic!("case {label:?} (create): answered no quickResponseId: {created:?}")
            });
        assert!(
            is_modelled_uuid(&quick_response_id),
            "case {label:?} (create): expected a modelled uuid quickResponseId, got \
             {quick_response_id:?}"
        );
        assert_reported_quick_response(
            &case,
            &created,
            &knowledge_base_id,
            &quick_response_id,
            "create",
        );

        // The same record, read back by id and by ARN, and through the parent's ARN form
        // too: a label form changes how the resource is addressed, never what it is.
        let quick_response_arn =
            expected_quick_response_arn(REGION, &knowledge_base_id, &quick_response_id);
        for (source, knowledge_base_label, quick_response_label) in [
            ("read by id", knowledge_base_id.as_str(), quick_response_id.as_str()),
            ("read by arn", knowledge_base_id.as_str(), quick_response_arn.as_str()),
            (
                "read through the parent's arn",
                knowledge_base_arn.as_str(),
                quick_response_id.as_str(),
            ),
        ] {
            let read = emulator
                .get_quick_response(knowledge_base_label, quick_response_label)
                .await;
            assert_reported_quick_response(
                &case,
                &read,
                &knowledge_base_id,
                &quick_response_id,
                source,
            );
        }
    }
}

#[tokio::test]
async fn name_and_shortcut_key_are_unique_inside_one_knowledge_base() {
    let emulator = Emulator::new();
    let first = emulator.knowledge_base_id("quick-response-conflicts").await;
    let second = emulator.knowledge_base_id("quick-response-conflicts-2").await;

    // Each row claims a name and a shortcut key; the clashing create below repeats one of
    // the two and varies the other, so the refusal is keyed to the member that clashes
    // rather than to the whole request, and names that member in its message.
    for (label, name, shortcut_key, clash_name, clash_shortcut_key, clashing_member) in [
        (
            "same name, different key",
            "reply-a",
            "/a",
            "reply-a",
            "/a2",
            "name",
        ),
        (
            "same key, different name",
            "reply-b",
            "/b",
            "reply-b2",
            "/b",
            "shortcut key",
        ),
        (
            "same name and key",
            "reply-c",
            "cccccccccc",
            "reply-c",
            "cccccccccc",
            "name",
        ),
    ] {
        let created = emulator
            .create_quick_response(
                &first,
                json!({
                    "name": name,
                    "content": { "content": "the first reply" },
                    "contentType": CONTENT_TYPE_PLAIN,
                    "shortcutKey": shortcut_key,
                }),
            )
            .await;
        let created_id = created
            .string("/quickResponse/quickResponseId")
            .unwrap_or_else(|| panic!("case {label:?}: the first create failed: {created:?}"));

        let clash = emulator
            .create_quick_response(
                &first,
                json!({
                    "name": clash_name,
                    "content": { "content": "the second reply" },
                    "contentType": CONTENT_TYPE_PLAIN,
                    "shortcutKey": clash_shortcut_key,
                }),
            )
            .await;
        assert_eq!(
            clash.status, 409,
            "case {label:?}: expected HTTP 409, got {} with body {}",
            clash.status, clash.body
        );
        assert!(
            clash.error_type.contains("ConflictException"),
            "case {label:?}: expected ConflictException, got {:?} with body {}",
            clash.error_type,
            clash.body
        );
        // The refusal states the member whose value is taken, as this endpoint words that
        // class.
        assert_eq!(
            clash.string("/message"),
            Some(format!(
                "The value for {clashing_member} is already in use. Please update and try again."
            )),
            "case {label:?}: expected the in-use message naming {clashing_member:?}, got body {}",
            clash.body
        );

        // The refusal left the first record exactly as it was.
        let still_there = emulator.get_quick_response(&first, &created_id).await;
        assert_eq!(
            still_there.string("/quickResponse/name").as_deref(),
            Some(name),
            "case {label:?}: the refused create must not touch the first quick response, got \
             body {}",
            still_there.body
        );
        assert_eq!(
            still_there.string("/quickResponse/shortcutKey").as_deref(),
            Some(shortcut_key),
            "case {label:?}: the refused create must not touch the first shortcutKey, got body \
             {}",
            still_there.body
        );

        // Uniqueness is scoped to ONE knowledge base: another knowledge base may hold the
        // same name and the same shortcut key.
        let elsewhere = emulator
            .create_quick_response(
                &second,
                json!({
                    "name": name,
                    "content": { "content": "a reply of another knowledge base" },
                    "contentType": CONTENT_TYPE_PLAIN,
                    "shortcutKey": shortcut_key,
                }),
            )
            .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {label:?}: another knowledge base may hold the same name and key, got HTTP {} \
             with body {}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.string("/quickResponse/quickResponseId"),
            Some(created_id.clone()),
            "case {label:?}: a create under another knowledge base must mint its own id"
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_answers_with_the_quick_response_it_created() {
    let emulator = Emulator::new();
    let first = emulator.knowledge_base_id("quick-response-idempotency").await;
    let second = emulator
        .knowledge_base_id("quick-response-idempotency-2")
        .await;

    for (index, (label, token)) in [
        ("a uuid-shaped token", "6a2f4b1e-0000-4000-8000-000000000001"),
        ("a token of one character", "t"),
        ("a token with punctuation", "retry::of/the-same#create"),
    ]
    .into_iter()
    .enumerate()
    {
        let body = json!({
            "name": format!("replayed-{index}"),
            "content": { "content": "the only reply this token creates" },
            "contentType": CONTENT_TYPE_PLAIN,
            "clientToken": token,
        });
        let created = emulator.create_quick_response(&first, body.clone()).await;
        let created_id = created
            .string("/quickResponse/quickResponseId")
            .unwrap_or_else(|| panic!("case {label:?}: the first create failed: {created:?}"));

        let replayed = emulator.create_quick_response(&first, body.clone()).await;
        assert_eq!(
            replayed.status, 200,
            "case {label:?}: a retry answers 200, got {} with body {}",
            replayed.status, replayed.body
        );
        assert_eq!(
            replayed.string("/quickResponse/quickResponseId").as_deref(),
            Some(created_id.as_str()),
            "case {label:?}: a replayed clientToken must answer with the quick response it \
             created, got {:?}",
            replayed.field("/quickResponse/quickResponseId")
        );

        // The token's scope is the knowledge base it created under, so the same token
        // under another one is a different create.
        let elsewhere = emulator.create_quick_response(&second, body).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {label:?}: the same token under another knowledge base creates, got HTTP {} \
             with body {}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.string("/quickResponse/quickResponseId"),
            Some(created_id),
            "case {label:?}: a create under another knowledge base must mint its own id"
        );
    }
}

#[tokio::test]
async fn a_label_naming_no_quick_response_of_this_knowledge_base_is_not_found() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base_id("quick-response-not-found").await;
    let other_knowledge_base_id = emulator
        .knowledge_base_id("quick-response-not-found-2")
        .await;

    let created = emulator
        .create_quick_response(
            &knowledge_base_id,
            json!({
                "name": "the only reply",
                "content": { "content": "hello" },
                "contentType": CONTENT_TYPE_PLAIN,
            }),
        )
        .await;
    let live_id = created
        .string("/quickResponse/quickResponseId")
        .expect("the create answered no quickResponseId");

    for (label, knowledge_base_label, quick_response_label, expected_message) in [
        (
            "bare uuid nothing holds",
            knowledge_base_id.clone(),
            ABSENT_UUID.to_string(),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
        ),
        (
            "this account's arn for an absent id",
            knowledge_base_id.clone(),
            expected_quick_response_arn(REGION, &knowledge_base_id, ABSENT_UUID),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
        ),
        (
            "another account's arn for a live id",
            knowledge_base_id.clone(),
            format!("arn:aws:wisdom:{REGION}:123456789012:quick-response/{knowledge_base_id}/{live_id}"),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
        ),
        (
            "another partition's arn for a live id",
            knowledge_base_id.clone(),
            format!(
                "arn:aws-cn:wisdom:{REGION}:{}:quick-response/{knowledge_base_id}/{live_id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
        ),
        (
            "a live id under another knowledge base",
            other_knowledge_base_id.clone(),
            live_id.clone(),
            MESSAGE_QUICK_RESPONSE_NOT_FOUND,
        ),
        (
            "a live id under a knowledge base nothing holds",
            ABSENT_UUID.to_string(),
            live_id.clone(),
            // The parent is what is missing here, so the parent is what is reported.
            MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
        ),
    ] {
        let reply = emulator
            .get_quick_response(&knowledge_base_label, &quick_response_label)
            .await;
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
            reply.string("/message").as_deref(),
            Some(expected_message),
            "case {label:?}: expected the sentence naming the missing resource, got body {}",
            reply.body
        );
    }

    // A create under a knowledge base this account does not hold is that same not-found,
    // and nothing is persisted for it.
    let orphan = emulator
        .create_quick_response(
            ABSENT_UUID,
            json!({
                "name": "a reply with no knowledge base",
                "content": { "content": "hello" },
                "contentType": CONTENT_TYPE_PLAIN,
            }),
        )
        .await;
    assert_eq!(
        orphan.status, 404,
        "case \"create under an absent knowledge base\": expected HTTP 404, got {} with body {}",
        orphan.status, orphan.body
    );
    assert!(
        orphan.error_type.contains("ResourceNotFoundException"),
        "case \"create under an absent knowledge base\": expected ResourceNotFoundException, got \
         {:?}",
        orphan.error_type
    );

    // The live quick response those labels named is untouched.
    let still_there = emulator
        .get_quick_response(&knowledge_base_id, &live_id)
        .await;
    assert_eq!(
        still_there.status, 200,
        "case \"live quick response after foreign-label addressing\": expected HTTP 200, got {} \
         with body {}",
        still_there.status, still_there.body
    );
}

#[tokio::test]
async fn deleting_the_knowledge_base_forgets_the_quick_responses_inside_it() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base_id("quick-response-reclaim").await;
    let survivor_knowledge_base_id = emulator.knowledge_base_id("quick-response-survivor").await;

    let mut doomed = Vec::new();
    for name in ["first", "second"] {
        let created = emulator
            .create_quick_response(
                &knowledge_base_id,
                json!({
                    "name": name,
                    "content": { "content": "a reply that goes with its knowledge base" },
                    "contentType": CONTENT_TYPE_PLAIN,
                    "clientToken": format!("token-of-{name}"),
                }),
            )
            .await;
        doomed.push(
            created
                .string("/quickResponse/quickResponseId")
                .unwrap_or_else(|| panic!("case {name:?}: the create failed: {created:?}")),
        );
    }
    let survivor = emulator
        .create_quick_response(
            &survivor_knowledge_base_id,
            json!({
                "name": "untouched",
                "content": { "content": "a reply of another knowledge base" },
                "contentType": CONTENT_TYPE_PLAIN,
            }),
        )
        .await;
    let survivor_id = survivor
        .string("/quickResponse/quickResponseId")
        .expect("the survivor create answered no quickResponseId");

    let deleted = emulator.delete_knowledge_base(&knowledge_base_id).await;
    assert_eq!(
        deleted.status, 204,
        "case \"delete the parent knowledge base\": expected the modelled HTTP 204, got {} with \
         body {}",
        deleted.status, deleted.body
    );

    for quick_response_id in &doomed {
        let reply = emulator
            .get_quick_response(&knowledge_base_id, quick_response_id)
            .await;
        assert_eq!(
            reply.status, 404,
            "case \"quick response of a deleted knowledge base\": expected HTTP 404 for \
             {quick_response_id}, got {} with body {}",
            reply.status, reply.body
        );
    }

    // Another knowledge base's quick response is untouched by that delete.
    let still_there = emulator
        .get_quick_response(&survivor_knowledge_base_id, &survivor_id)
        .await;
    assert_eq!(
        still_there.status, 200,
        "case \"quick response of another knowledge base\": expected HTTP 200, got {} with body \
         {}",
        still_there.status, still_there.body
    );
}

#[tokio::test]
async fn the_tags_a_create_persisted_are_what_list_tags_for_resource_reports() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base_id("quick-response-tags").await;

    for (label, tags) in [
        ("no tags at all", BTreeMap::new()),
        (
            "one tag",
            [("env".to_string(), "test".to_string())]
                .into_iter()
                .collect::<BTreeMap<String, String>>(),
        ),
        (
            "several tags",
            [
                ("a.b".to_string(), "1".to_string()),
                ("team_1".to_string(), "alpha".to_string()),
                ("owner".to_string(), "quick-responses".to_string()),
            ]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        ),
    ] {
        let mut body = json!({
            "name": format!("tagged-{label}").replace(' ', "-"),
            "content": { "content": "a tagged reply" },
            "contentType": CONTENT_TYPE_PLAIN,
        });
        if !tags.is_empty() {
            body["tags"] = json!(tags);
        }
        let created = emulator.create_quick_response(&knowledge_base_id, body).await;
        let arn = created
            .string("/quickResponse/quickResponseArn")
            .unwrap_or_else(|| panic!("case {label:?}: the create failed: {created:?}"));

        let listed = emulator.list_tags(&arn).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: expected HTTP 200 from ListTagsForResource, got {} with body {}",
            listed.status, listed.body
        );
        let reported: BTreeMap<String, String> = listed
            .field("/tags")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported, tags,
            "case {label:?}: expected the tags the create persisted, got body {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn the_listing_pages_the_quick_responses_of_one_knowledge_base() {
    let emulator = Emulator::new();
    let listed = emulator.knowledge_base_id("quick-response-listing").await;
    let other = emulator.knowledge_base_id("quick-response-listing-2").await;

    // Several quick responses in the listed knowledge base and one in another, so a page
    // is scoped to the parent the route names.
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        let reply = emulator
            .create_quick_response(
                &listed,
                json!({
                    "name": format!("listed-{index}"),
                    "content": { "content": "a listed reply" },
                    "contentType": CONTENT_TYPE_PLAIN,
                }),
            )
            .await;
        created.push(
            reply
                .string("/quickResponse/quickResponseId")
                .unwrap_or_else(|| panic!("case \"listed-{index}\": the create failed: {reply:?}")),
        );
    }
    created.sort();
    let elsewhere = emulator
        .create_quick_response(
            &other,
            json!({
                "name": "not listed here",
                "content": { "content": "a reply of another knowledge base" },
                "contentType": CONTENT_TYPE_PLAIN,
            }),
        )
        .await;
    let elsewhere_id = elsewhere
        .string("/quickResponse/quickResponseId")
        .expect("the create of the other knowledge base's reply failed");

    // Every page size — one row at a time, a few rows, exactly the listing's length, more
    // than it holds, and none at all — walks the same ids in `quickResponseId` order,
    // each exactly once.
    for page_size in [Some(1), Some(2), Some(5), Some(100), None] {
        let case = format!("maxResults {page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = emulator
                .list_quick_responses(&listed, page_size, token.as_deref())
                .await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                page.status, page.body
            );
            let summaries = page
                .field("/quickResponseSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(size) = page_size {
                assert!(
                    summaries.len() <= size,
                    "case {case:?}: a page holds at most maxResults entries, got {}",
                    summaries.len()
                );
            }
            for summary in &summaries {
                let field = |name: &str| summary.get(name).and_then(Value::as_str);
                let id = field("quickResponseId").unwrap_or_default().to_string();
                assert_eq!(
                    field("knowledgeBaseId"),
                    Some(listed.as_str()),
                    "case {case:?}: every entry belongs to the knowledge base listed, got \
                     {summary}"
                );
                assert_eq!(
                    field("quickResponseArn"),
                    Some(expected_quick_response_arn(REGION, &listed, &id).as_str()),
                    "case {case:?}: expected the stored ARN, got {summary}"
                );
                assert_eq!(
                    field("status"),
                    Some(STATUS_CREATED),
                    "case {case:?}: expected status {STATUS_CREATED}, got {summary}"
                );
                assert_eq!(
                    field("contentType"),
                    Some(CONTENT_TYPE_PLAIN),
                    "case {case:?}: expected the stored contentType, got {summary}"
                );
                assert!(
                    summary
                        .get("createdTime")
                        .and_then(Value::as_f64)
                        .is_some_and(|seconds| seconds > 0.0),
                    "case {case:?}: expected a createdTime in epoch seconds, got {summary}"
                );
                walked.push(id);
            }
            token = page.string("/nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, created,
            "case {case:?}: the listing walks every quick response of the knowledge base in id \
             order, got {walked:?}"
        );
        assert!(
            !walked.contains(&elsewhere_id),
            "case {case:?}: another knowledge base's quick response must not be listed"
        );
    }

    // A knowledge base this account does not hold is the not-found, never an empty page.
    let absent = emulator.list_quick_responses(ABSENT_UUID, None, None).await;
    assert_eq!(
        absent.status, 404,
        "case \"listing of an absent knowledge base\": expected HTTP 404, got {} with body {}",
        absent.status, absent.body
    );
    assert!(
        absent.error_type.contains("ResourceNotFoundException"),
        "case \"listing of an absent knowledge base\": expected ResourceNotFoundException, got \
         {:?}",
        absent.error_type
    );

    // A token this listing could not have issued names no position, so it is refused
    // rather than read as "from the beginning".
    let bad_token = emulator
        .list_quick_responses(&listed, None, Some("not-a-token"))
        .await;
    assert_eq!(
        bad_token.status, 400,
        "case \"listing with a token this service did not mint\": expected HTTP 400, got {} with \
         body {}",
        bad_token.status, bad_token.body
    );
    assert!(
        bad_token.error_type.contains("ValidationException"),
        "case \"listing with a token this service did not mint\": expected ValidationException, \
         got {:?}",
        bad_token.error_type
    );
}

#[tokio::test]
async fn one_knowledge_base_holds_at_most_the_quick_response_ceiling() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.knowledge_base_id("quick-response-ceiling").await;

    for index in 0..MAX_QUICK_RESPONSES_PER_KNOWLEDGE_BASE {
        let reply = emulator
            .create_quick_response(
                &knowledge_base_id,
                json!({
                    "name": format!("reply-{index}"),
                    "content": { "content": "a reply inside the ceiling" },
                    "contentType": CONTENT_TYPE_PLAIN,
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"create {index} of the ceiling\": expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
    }

    let past_the_ceiling = emulator
        .create_quick_response(
            &knowledge_base_id,
            json!({
                "name": "one too many",
                "content": { "content": "a reply past the ceiling" },
                "contentType": CONTENT_TYPE_PLAIN,
            }),
        )
        .await;
    assert_eq!(
        past_the_ceiling.status, 402,
        "case \"create past the ceiling\": expected the modelled HTTP 402, got {} with body {}",
        past_the_ceiling.status, past_the_ceiling.body
    );
    assert!(
        past_the_ceiling
            .error_type
            .contains("ServiceQuotaExceededException"),
        "case \"create past the ceiling\": expected ServiceQuotaExceededException, got {:?} with \
         body {}",
        past_the_ceiling.error_type,
        past_the_ceiling.body
    );
    // The refusal states the ceiling and what to do about it, and nothing about what the
    // knowledge base already holds.
    assert_eq!(
        past_the_ceiling.string("/message"),
        Some(format!(
            "The maximum number of quick responses ({MAX_QUICK_RESPONSES_PER_KNOWLEDGE_BASE}) in \
             this knowledge base has been reached. Delete a quick response before creating \
             another."
        )),
        "case \"create past the ceiling\": expected the quota sentence, got body {}",
        past_the_ceiling.body
    );
}
