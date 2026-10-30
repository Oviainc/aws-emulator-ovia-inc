// SPDX-License-Identifier: CC0-1.0

//! How this endpoint CLASSIFIES and WORDS the client errors it answers, driven over HTTP
//! through the same router the emulator serves with.
//!
//! The two rules under test, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! * a value a modelled enumeration does not declare — which the operation's input shape
//!   cannot hold at all — is refused where the request body is READ, before any handler
//!   runs, with HTTP 400, the error code `BadRequestException` and one fixed sentence
//!   (`Invalid request body`) that carries nothing from the request: not the offending
//!   value, not the member's name and not the value set. Nothing is persisted by a request
//!   refused there. Every other refusal class keeps the error this model declares
//!   (`ValidationException`) with the constraint-stating sentence AWS words it with;
//! * the TAGGING surface answers a `resourceArn` that names no resource of this account
//!   with HTTP 404, the error code [`TAGGING_NOT_FOUND_CODE`] and one fixed sentence
//!   ([`state::MESSAGE_TAGGING_RESOURCE_NOT_FOUND`]), for every one of its three
//!   operations — and leaves the resources the account does hold exactly as they were.
//!
//! The enum rows differ along what the rule generalises over: which enumeration is
//! violated (a one-value one, a multi-value one), where the member sits (the request's top
//! level, inside a list element, inside a structure inside a structure), whether the
//! operation is implemented or still scaffolded, and the FORM of the rejected value (an
//! unknown token, a declared value in the wrong case, a value carrying the error
//! envelope's own metacharacters). The tagging rows differ along the operation called and
//! along what makes the ARN miss (the id, the family label, the region, the account, the
//! partition, and a resource a delete removed).
//!
//! This view of the shared `wisdom` endpoint answers under its own registration key (see
//! `src/service.rs`), which is what every request below is signed for. The harness — the
//! router, the creates and the ARN forms — is the one `resource_tags.rs` already drives
//! this surface with.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::state;

use super::resource_tags::{
    label, tag_map, wisdom_arn, Emulator, Reply, ABSENT_UUID, OTHER_ACCOUNT, OTHER_REGION, REGION,
    TAGGING_NOT_FOUND_CODE,
};

/// The error code the boundary that reads a request body answers a value no modelled
/// enumeration declares with. It is not a modelled operation error
/// (`smithy_query(query="operation",operation="CreateAssistant")` declares
/// `AccessDeniedException`, `ConflictException`, `ServiceQuotaExceededException` and
/// `ValidationException`), because the refusal belongs to that boundary.
const BAD_REQUEST: &str = "BadRequestException";

/// The one sentence that boundary refuses a body it will not read with.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The error code every class this service validates ITSELF keeps — the one its model
/// declares for input it validates.
const VALIDATION: &str = "ValidationException";

/// An example `UuidOrArn` path label: the form every id label in this model takes. The
/// operations the enum rows address are refused before any lookup, so the label need name
/// no stored resource.
const ID: &str = "11111111-1111-1111-1111-111111111111";

/// One refusal row: the case, the method, the URI, the JSON body, and the error code the
/// answer must carry.
type RefusalRow = (&'static str, &'static str, String, Value, &'static str);

#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused_as_a_bad_request() {
    let emulator = Emulator::new();

    // The value sets are the model's own (`smithy_query(query="shape",shape=…)`):
    // `AssistantType` [AGENT]; `KnowledgeBaseType` [EXTERNAL, CUSTOM, QUICK_RESPONSES];
    // `AssociationType` [KNOWLEDGE_BASE]; `FilterField` [NAME]; `FilterOperator` [EQUALS];
    // `QuickResponseFilterOperator` [EQUALS, PREFIX]; `Order` [ASC, DESC].
    let rows: Vec<RefusalRow> = vec![
        (
            "CreateAssistant with a type outside AssistantType (one-value enum)",
            "POST",
            "/assistants".to_string(),
            json!({ "name": "enum-row-1", "type": "NOT_A_REAL_ASSISTANT_TYPE" }),
            BAD_REQUEST,
        ),
        (
            "CreateAssistant with the declared AssistantType value in the wrong case",
            "POST",
            "/assistants".to_string(),
            json!({ "name": "enum-row-2", "type": "agent" }),
            BAD_REQUEST,
        ),
        (
            "CreateKnowledgeBase with a knowledgeBaseType outside KnowledgeBaseType \
             (multi-value enum)",
            "POST",
            "/knowledgeBases".to_string(),
            json!({ "name": "enum-row-3", "knowledgeBaseType": "NOT_A_KNOWLEDGE_BASE_TYPE" }),
            BAD_REQUEST,
        ),
        (
            "CreateAssistantAssociation with an associationType outside AssociationType",
            "POST",
            format!("/assistants/{ID}/associations"),
            json!({
                "associationType": "NOT_AN_ASSOCIATION_TYPE",
                "association": { "knowledgeBaseId": ID },
            }),
            BAD_REQUEST,
        ),
        (
            "SearchContent with a filter field outside FilterField, inside a list element",
            "POST",
            format!("/knowledgeBases/{ID}/search"),
            json!({
                "searchExpression": {
                    "filters": [
                        { "field": "NOT_A_FILTER_FIELD", "operator": "EQUALS", "value": "x" }
                    ]
                }
            }),
            BAD_REQUEST,
        ),
        (
            "SearchContent with a filter operator outside FilterOperator",
            "POST",
            format!("/knowledgeBases/{ID}/search"),
            json!({
                "searchExpression": {
                    "filters": [
                        { "field": "NAME", "operator": "NOT_AN_OPERATOR", "value": "x" }
                    ]
                }
            }),
            BAD_REQUEST,
        ),
        (
            "SearchQuickResponses with an operator outside QuickResponseFilterOperator",
            "POST",
            format!("/knowledgeBases/{ID}/search/quickResponses"),
            json!({
                "searchExpression": {
                    "filters": [
                        { "name": "name", "operator": "NOT_AN_OPERATOR", "values": ["x"] }
                    ]
                }
            }),
            BAD_REQUEST,
        ),
        (
            "SearchQuickResponses with an order outside Order, nested two structures deep",
            "POST",
            format!("/knowledgeBases/{ID}/search/quickResponses"),
            json!({
                "searchExpression": {
                    "orderOnField": { "name": "name", "order": "SIDEWAYS" }
                }
            }),
            BAD_REQUEST,
        ),
        (
            // The refused value must NOT travel into the answer, so a value carrying the
            // error envelope's own metacharacters still comes back as a well-formed body
            // stating the one sentence, with nothing of the request in it.
            "CreateAssistant with a type that carries JSON envelope metacharacters",
            "POST",
            "/assistants".to_string(),
            json!({
                "name": "enum-row-9",
                "type": "\"}, \"__type\": \"Spoofed\", \"x\": \"",
            }),
            BAD_REQUEST,
        ),
        (
            // A request that ALSO breaks a constraint this service validates itself could
            // not be read into the input shape either, so the boundary's refusal is the
            // one the caller gets.
            "CreateAssistant breaking BOTH the enum and the modelled Name pattern",
            "POST",
            "/assistants".to_string(),
            json!({ "name": "!!!", "type": "NOT_A_REAL_ASSISTANT_TYPE" }),
            BAD_REQUEST,
        ),
    ];

    for (case, method, uri, body, expected_code) in rows {
        let reply = emulator.send(method, &uri, Some(body)).await;
        assert_eq!(
            (reply.status, reply.error_type.as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {} {:?} with {}",
            reply.status,
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message"),
            INVALID_REQUEST_BODY,
            "case {case:?}: expected the one sentence this class states, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_constraint_this_service_validates_itself_keeps_its_modelled_error() {
    let emulator = Emulator::new();

    // The controls for the rule above: a body the schema admits is not refused at all, and
    // every other refusal class — an absent `@required` member, a `@pattern`, a `@range` —
    // keeps the code this model declares and a message that STATES the constraint rather
    // than the one sentence the body boundary answers with.
    let rows: Vec<RefusalRow> = vec![
        (
            "CreateAssistant with the AssistantType value the model declares",
            "POST",
            "/assistants".to_string(),
            json!({ "name": "control-row-1", "type": "AGENT" }),
            "",
        ),
        (
            "CreateKnowledgeBase with a KnowledgeBaseType value the model declares",
            "POST",
            "/knowledgeBases".to_string(),
            json!({ "name": "control-row-2", "knowledgeBaseType": "CUSTOM" }),
            "",
        ),
        (
            // `Name`'s pattern is `^[a-zA-Z0-9\s_.,-]+`, anchored at the start, so a name
            // whose first character is outside it matches nowhere.
            "CreateAssistant with a name outside the modelled Name pattern",
            "POST",
            "/assistants".to_string(),
            json!({ "name": "!!!", "type": "AGENT" }),
            VALIDATION,
        ),
        (
            "CreateAssistant with the required type absent",
            "POST",
            "/assistants".to_string(),
            json!({ "name": "control-row-4" }),
            VALIDATION,
        ),
        (
            "CreateKnowledgeBase with the required knowledgeBaseType absent",
            "POST",
            "/knowledgeBases".to_string(),
            json!({ "name": "control-row-5" }),
            VALIDATION,
        ),
    ];

    for (case, method, uri, body, expected_code) in rows {
        let reply = emulator.send(method, &uri, Some(body)).await;
        assert_eq!(
            reply.error_type, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got HTTP {} {:?} \
             with {}",
            reply.status, reply.error_type, reply.body
        );
        if expected_code.is_empty() {
            continue;
        }
        assert_ne!(
            reply.string("/message"),
            INVALID_REQUEST_BODY,
            "case {case:?}: a class this service validates itself must state the \
             constraint, not the body boundary's sentence, got {}",
            reply.body
        );
    }
}

/// A request the body boundary refuses PERSISTS nothing: the name it carried is still free
/// afterwards, so a later create that satisfies the schema is not a conflict.
#[tokio::test]
async fn a_refused_create_persists_nothing() {
    let emulator = Emulator::new();
    let name = "refused-then-created";

    let refused = emulator
        .send(
            "POST",
            "/assistants",
            Some(json!({ "name": name, "type": "NOT_A_REAL_ASSISTANT_TYPE" })),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_str()),
        (400, BAD_REQUEST),
        "case \"a refused create\": expected HTTP 400 {BAD_REQUEST}, got {} {:?} with {}",
        refused.status,
        refused.error_type,
        refused.body
    );

    let listed = emulator.send("GET", "/assistants", None).await;
    assert_eq!(
        listed.status, 200,
        "case \"a refused create\": expected the listing to answer HTTP 200, got {} with {}",
        listed.status, listed.body
    );
    let names: Vec<String> = listed
        .field("/assistantSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.is_empty(),
        "case \"a refused create\": the refused request must have stored no assistant, \
         got {names:?}"
    );

    // And the name is still free: the create the schema admits succeeds rather than
    // conflicting with a record the refused request left behind.
    let created = emulator
        .send(
            "POST",
            "/assistants",
            Some(json!({ "name": name, "type": "AGENT" })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"the create the schema admits\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
}

/// RULE: the tagging surface answers a `resourceArn` naming no resource of this account
/// with one code and one sentence, whichever of its three operations was called, and
/// changes nothing about the resources the account does hold.
#[tokio::test]
async fn the_tagging_surface_words_an_arn_it_cannot_resolve_one_way() {
    let emulator = Emulator::new();
    let assistant = emulator
        .create_assistant("tagging-wording-assistant", &tag_map(1))
        .await;
    assert_eq!(
        assistant.status, 200,
        "case \"the host assistant\": expected HTTP 200, got {} with {}",
        assistant.status, assistant.body
    );
    let assistant_id = assistant.string("/assistant/assistantId");
    let assistant_arn = assistant.string("/assistant/assistantArn");
    let knowledge_base = emulator
        .create_knowledge_base("tagging-wording-kb", &BTreeMap::new())
        .await;
    let knowledge_base_id = knowledge_base.string("/knowledgeBase/knowledgeBaseId");
    let knowledge_base_arn = knowledge_base.string("/knowledgeBase/knowledgeBaseArn");
    let account = engine::EMULATOR_ACCOUNT_ID;

    // The ARNs that name nothing: the fabricated id a recorded client builds by replacing
    // a real resource's id, a stored id under the wrong family label, and the same stored
    // resource addressed in another region, account or partition.
    let mut misses = vec![
        (
            "the right region and account, an id nothing holds",
            wisdom_arn("aws", REGION, account, &format!("assistant/{ABSENT_UUID}")),
        ),
        (
            "a stored assistant's id under another family's label",
            wisdom_arn(
                "aws",
                REGION,
                account,
                &format!("knowledge-base/{assistant_id}"),
            ),
        ),
        (
            "a stored assistant in another region",
            wisdom_arn(
                "aws",
                OTHER_REGION,
                account,
                &format!("assistant/{assistant_id}"),
            ),
        ),
        (
            "a stored assistant under another account",
            wisdom_arn(
                "aws",
                REGION,
                OTHER_ACCOUNT,
                &format!("assistant/{assistant_id}"),
            ),
        ),
        (
            "a stored assistant in another partition",
            wisdom_arn(
                "aws-cn",
                REGION,
                account,
                &format!("assistant/{assistant_id}"),
            ),
        ),
    ];
    // A resource a DELETE removed holds no tags any more, so its ARN names nothing either.
    assert_eq!(
        emulator
            .delete_knowledge_base(&knowledge_base_id)
            .await
            .status,
        204,
        "case \"delete the knowledge base\": expected the modelled 204"
    );
    misses.push(("a knowledge base a delete removed", knowledge_base_arn));

    for (case, arn) in &misses {
        for (operation, reply) in [
            ("ListTagsForResource", emulator.list_tags(arn).await),
            ("TagResource", tag_resource(&emulator, arn).await),
            ("UntagResource", untag_resource(&emulator, arn).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case:?} ({operation}): expected HTTP 404 for {arn}, got {} with {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type, TAGGING_NOT_FOUND_CODE,
                "case {case:?} ({operation}): expected the code {TAGGING_NOT_FOUND_CODE}, \
                 got {:?} with {}",
                reply.error_type, reply.body
            );
            assert_eq!(
                reply.string("/message"),
                state::MESSAGE_TAGGING_RESOURCE_NOT_FOUND,
                "case {case:?} ({operation}): expected the sentence this surface words \
                 that class with, got {}",
                reply.body
            );
            assert_eq!(
                reply.field("/resourceName"),
                None,
                "case {case:?} ({operation}): the refusal must carry nothing from the \
                 request, got {}",
                reply.body
            );
        }
    }

    // The resource the account DOES hold was left exactly as it was by every refusal
    // above, and tagging its own ARN still works and reads back.
    let listed = emulator.list_tags(&assistant_arn).await;
    assert_eq!(
        (listed.status, listed.field("/tags").cloned()),
        (200, Some(json!(tag_map(1)))),
        "case \"the resource this account holds\": expected its create-time tags, got {} \
         with {}",
        listed.status,
        listed.body
    );
    let tagged = tag_resource(&emulator, &assistant_arn).await;
    assert_eq!(
        tagged.status, 204,
        "case \"tagging the resource this account holds\": expected the no-content HTTP 204, \
         got {} with {}",
        tagged.status, tagged.body
    );
    let listed = emulator.list_tags(&assistant_arn).await;
    let mut want = tag_map(1);
    want.insert(MERGED_KEY.to_string(), MERGED_VALUE.to_string());
    assert_eq!(
        (listed.status, listed.field("/tags").cloned()),
        (200, Some(json!(want))),
        "case \"tagging the resource this account holds\": expected the merged map read \
         back, got {} with {}",
        listed.status,
        listed.body
    );
}

/// The one pair the tagging rows merge, so the read-back assertion names what it expects.
const MERGED_KEY: &str = "stage";
const MERGED_VALUE: &str = "probe";

/// `TagResource` on one ARN with one pair.
async fn tag_resource(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/tags/{}", label(resource_arn)),
            Some(json!({ "tags": { MERGED_KEY: MERGED_VALUE } })),
        )
        .await
}

/// `UntagResource` on one ARN with one tag key.
async fn untag_resource(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .send(
            "DELETE",
            &format!("/tags/{}?tagKeys={MERGED_KEY}", label(resource_arn)),
            None,
        )
        .await
}
