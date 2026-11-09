// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Q in Connect AI-guardrail slice
//! (`CreateAIGuardrail` + `GetAIGuardrail` + `DeleteAIGuardrail`), driven over HTTP
//! through the same router the emulator serves with.
//!
//! The rule under test, asserted over a TABLE of inputs rather than the one recorded
//! request: a create of a resource nested in an assistant PERSISTS every member the
//! request carried and answers out of the stored record, so a later read — by id or by
//! ARN — reports exactly the same values, and a delete forgets that record, after which a
//! read of it is not found and the name it held is free again.
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * both values the modelled `VisibilityStatus` declares;
//! * the published length bounds of `name`, `description` and both blocked-messaging
//!   members, and whether the optional members are sent at all;
//! * each of the five policy configurations the create accepts, alone and all together,
//!   spanning every value of the enumerations they carry (content filter types and
//!   strengths, the topic type, the managed-word list, sensitive-information actions and
//!   both contextual-grounding filter types, including the published minimum threshold);
//! * the tag count (none, one, several).
//!
//! Every assertion goes over the wire — only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected — so a response that is not computed from
//! persisted state cannot pass. The harness (the router, the reply reader, the assistant
//! create and the tagging read) is the one `resource_tags.rs` already drives this service
//! with.

use serde_json::{json, Value};

use service_com_amazonaws_qconnect_wisdomservice::state::{
    MAX_AI_GUARDRAILS_PER_ASSISTANT, MAX_AI_GUARDRAIL_POLICY_ENTRIES,
};

use super::resource_tags::{
    label, tag_map, wisdom_arn, Emulator, Reply, ABSENT_UUID, OTHER_ACCOUNT, REGION,
};

/// Both values the modelled `VisibilityStatus` declares
/// (`smithy_query(query="shape",shape="VisibilityStatus")`), written out here rather than
/// imported from the implementation so the table spans the whole enumeration.
pub(crate) const VISIBILITY_SAVED: &str = "SAVED";
pub(crate) const VISIBILITY_PUBLISHED: &str = "PUBLISHED";

/// The floating version alias the working copy of a versioned AI resource is named by
/// (the model's `UuidOrArnOrEitherWithQualifier` admits `:<QUALIFIER>`,
/// `smithy_query(query="shape",shape="UuidOrArnOrEitherWithQualifier")`), written out here
/// so the assertions pin the qualifier a client sees.
pub(crate) const QUALIFIER_LATEST: &str = "$LATEST";

/// The `Status` an AI guardrail this service holds reports
/// (`smithy_query(query="shape",shape="Status")` declares it beside the transient
/// `CREATE_IN_PROGRESS`, `DELETE_IN_PROGRESS` and the rest).
pub(crate) const STATUS_ACTIVE: &str = "ACTIVE";

/// The published bounds of the members this suite varies in length
/// (`smithy_query(query="operation",operation="CreateAIGuardrail")`: `name` is `len
/// 1..255`, `description` `len 1..200`, and both blocked-messaging members `len 1..500`).
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 255;
pub(crate) const DESCRIPTION_MIN: usize = 1;
pub(crate) const DESCRIPTION_MAX: usize = 200;
pub(crate) const MESSAGING_MIN: usize = 1;
pub(crate) const MESSAGING_MAX: usize = 500;

/// Every value of the enumerations the content policy carries
/// (`smithy_query(query="shape",shape="GuardrailContentFilterType")` and
/// `…shape="GuardrailFilterStrength"`), so the row that configures it spans the catalog
/// instead of the one filter a single recording used. `filtersConfig` admits at most six
/// entries, which is exactly the number of filter types.
pub(crate) const CONTENT_FILTER_TYPES: [&str; 6] = [
    "SEXUAL",
    "VIOLENCE",
    "HATE",
    "INSULTS",
    "MISCONDUCT",
    "PROMPT_ATTACK",
];
pub(crate) const FILTER_STRENGTHS: [&str; 4] = ["NONE", "LOW", "MEDIUM", "HIGH"];

/// The only value the modelled `GuardrailTopicType` declares, and the only
/// `GuardrailManagedWordsType`
/// (`smithy_query(query="shape",shape="GuardrailTopicType")`,
/// `…shape="GuardrailManagedWordsType"`).
pub(crate) const TOPIC_TYPE_DENY: &str = "DENY";
pub(crate) const MANAGED_WORDS_PROFANITY: &str = "PROFANITY";

/// Both values the modelled `GuardrailSensitiveInformationAction` declares, and both
/// `GuardrailContextualGroundingFilterType` values
/// (`smithy_query(query="shape",shape="GuardrailSensitiveInformationAction")`,
/// `…shape="GuardrailContextualGroundingFilterType"`).
pub(crate) const ACTIONS: [&str; 2] = ["BLOCK", "ANONYMIZE"];
pub(crate) const GROUNDING_TYPES: [&str; 2] = ["GROUNDING", "RELEVANCE"];

/// PII entity types the sensitive-information rows configure: entries of the modelled
/// `GuardrailPiiEntityType` catalog (`smithy_query(query="shape",shape="GuardrailPiiEntityType")`
/// declares thirty-one), including ones no recording of this surface used.
pub(crate) const PII_ENTITY_TYPES: [&str; 3] = ["EMAIL", "PHONE", "US_SOCIAL_SECURITY_NUMBER"];

/// The published minimum of `GuardrailContextualGroundingFilterConfig$threshold`
/// (`smithy_query(query="shape",shape="GuardrailContextualGroundingFilterConfig")`:
/// `range 0..`), and one value above it.
pub(crate) const THRESHOLD_MIN: f64 = 0.0;
pub(crate) const THRESHOLD_ABOVE_MIN: f64 = 0.75;

/// The fixed sentences this endpoint answers a resource it does not hold with, and the
/// ones it refuses a conflicting create with. Written out here rather than imported from
/// the implementation, so the assertions pin the answer a client sees.
pub(crate) const MESSAGE_AI_GUARDRAIL_NOT_FOUND: &str = "AI Guardrail does not exist.";
pub(crate) const MESSAGE_ASSISTANT_NOT_FOUND: &str = "Assistant does not exist.";
const MESSAGE_NAME_IN_USE: &str = "Name is already in use";
pub(crate) const MESSAGE_IDEMPOTENCY_CONFLICT: &str = "Idempotency conflict";

/// `CreateAIGuardrail` on one assistant, as the model's URI binds it
/// (`POST /assistants/{assistantId}/aiguardrails`).
pub(crate) async fn create_guardrail(emulator: &Emulator, assistant: &str, body: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/assistants/{}/aiguardrails", label(assistant)),
            Some(body),
        )
        .await
}

/// `GetAIGuardrail` (`GET /assistants/{assistantId}/aiguardrails/{aiGuardrailId}`).
pub(crate) async fn get_guardrail(emulator: &Emulator, assistant: &str, guardrail: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!(
                "/assistants/{}/aiguardrails/{}",
                label(assistant),
                label(guardrail)
            ),
            None,
        )
        .await
}

/// `DeleteAIGuardrail` (`DELETE /assistants/{assistantId}/aiguardrails/{aiGuardrailId}`).
pub(crate) async fn delete_guardrail(
    emulator: &Emulator,
    assistant: &str,
    guardrail: &str,
) -> Reply {
    emulator
        .send(
            "DELETE",
            &format!(
                "/assistants/{}/aiguardrails/{}",
                label(assistant),
                label(guardrail)
            ),
            None,
        )
        .await
}

/// A fresh assistant to nest guardrails in, answering with the id its create minted.
pub(crate) async fn assistant_id(emulator: &Emulator, name: &str) -> String {
    let reply = emulator.create_assistant(name, &tag_map(0)).await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: the assistant this suite nests under must be created, got HTTP {} \
         with body {}",
        reply.status, reply.body
    );
    let id = reply.string("/assistant/assistantId");
    assert!(
        !id.is_empty(),
        "case {name:?}: the assistant create answered no assistantId, body {}",
        reply.body
    );
    id
}

/// The required members of every create in this suite, so a row states only what it
/// varies.
pub(crate) fn required(name: &str, visibility: &str) -> Value {
    json!({
        "name": name,
        "visibilityStatus": visibility,
        "blockedInputMessaging": "This prompt was blocked.",
        "blockedOutputsMessaging": "This response was blocked.",
    })
}

/// One row of the create table: what it varies, and the request body it sends.
#[derive(Debug, Clone)]
struct Case {
    label: String,
    body: Value,
}

/// The create bodies the round-trip rule is asserted over. Each row differs from the
/// others along one axis of the rule — the visibility status, a published length bound, an
/// optional member, a policy configuration, the tag count — and every row's `name` is
/// unique inside the assistant, since a name identifies a guardrail there.
fn cases() -> Vec<Case> {
    let content_filters: Vec<Value> = CONTENT_FILTER_TYPES
        .iter()
        .enumerate()
        .map(|(index, filter)| {
            json!({
                "type": filter,
                "inputStrength": FILTER_STRENGTHS[index % FILTER_STRENGTHS.len()],
                "outputStrength": FILTER_STRENGTHS[(index + 1) % FILTER_STRENGTHS.len()],
            })
        })
        .collect();
    let pii_entities: Vec<Value> = PII_ENTITY_TYPES
        .iter()
        .enumerate()
        .map(|(index, entity)| {
            json!({ "type": entity, "action": ACTIONS[index % ACTIONS.len()] })
        })
        .collect();
    let grounding_filters: Vec<Value> = GROUNDING_TYPES
        .iter()
        .enumerate()
        .map(|(index, filter)| {
            json!({
                "type": filter,
                "threshold": if index == 0 { THRESHOLD_MIN } else { THRESHOLD_ABOVE_MIN },
            })
        })
        .collect();
    let topic_policy = json!({
        "topicsConfig": [
            {
                "name": "Investment advice",
                "definition": "Any request for personalised investment recommendations.",
                "examples": ["Which stock should I buy?", "Is this fund a good investment?"],
                "type": TOPIC_TYPE_DENY,
            },
            {
                "name": "Legal advice",
                "definition": "Any request for legal counsel.",
                "type": TOPIC_TYPE_DENY,
            },
        ],
    });
    let word_policy_words = json!({ "wordsConfig": [{ "text": "forbidden" }, { "text": "banned" }] });
    let word_policy_managed =
        json!({ "managedWordListsConfig": [{ "type": MANAGED_WORDS_PROFANITY }] });
    let sensitive_regexes = json!({
        "regexesConfig": [
            {
                "name": "employee id",
                "description": "An internal employee identifier.",
                "pattern": "EMP-[0-9]{6}",
                "action": ACTIONS[0],
            },
            { "name": "ticket id", "pattern": "TCK-[0-9]+", "action": ACTIONS[1] },
        ],
    });

    let mut cases = vec![
        Case {
            label: "only the required members, visibility SAVED".to_string(),
            body: required("saved only", VISIBILITY_SAVED),
        },
        Case {
            label: "only the required members, visibility PUBLISHED".to_string(),
            body: required("published only", VISIBILITY_PUBLISHED),
        },
        Case {
            label: format!("name at its published minimum length ({NAME_MIN})"),
            body: required(&"n".repeat(NAME_MIN), VISIBILITY_SAVED),
        },
        Case {
            label: format!("name at its published maximum length ({NAME_MAX})"),
            body: required(&"n".repeat(NAME_MAX), VISIBILITY_PUBLISHED),
        },
    ];

    let mut with = |label: &str, name: &str, visibility: &str, members: Vec<(&str, Value)>| {
        let mut body = required(name, visibility);
        for (member, value) in members {
            body[member] = value;
        }
        cases.push(Case {
            label: label.to_string(),
            body,
        });
    };

    with(
        &format!("blocked messaging at both published minimum lengths ({MESSAGING_MIN})"),
        "messaging at minimum",
        VISIBILITY_SAVED,
        vec![
            ("blockedInputMessaging", json!("b".repeat(MESSAGING_MIN))),
            ("blockedOutputsMessaging", json!("o".repeat(MESSAGING_MIN))),
        ],
    );
    with(
        &format!("blocked messaging at both published maximum lengths ({MESSAGING_MAX})"),
        "messaging at maximum",
        VISIBILITY_PUBLISHED,
        vec![
            ("blockedInputMessaging", json!("b".repeat(MESSAGING_MAX))),
            ("blockedOutputsMessaging", json!("o".repeat(MESSAGING_MAX))),
        ],
    );
    with(
        &format!("description at its published minimum length ({DESCRIPTION_MIN})"),
        "description at minimum",
        VISIBILITY_SAVED,
        vec![("description", json!("d".repeat(DESCRIPTION_MIN)))],
    );
    with(
        &format!("description at its published maximum length ({DESCRIPTION_MAX})"),
        "description at maximum",
        VISIBILITY_PUBLISHED,
        vec![("description", json!("d".repeat(DESCRIPTION_MAX)))],
    );
    with(
        "a topic policy, one entry with examples and one without",
        "topic policy",
        VISIBILITY_SAVED,
        vec![("topicPolicyConfig", topic_policy.clone())],
    );
    with(
        "a content policy naming every filter type and strength",
        "content policy",
        VISIBILITY_SAVED,
        vec![(
            "contentPolicyConfig",
            json!({ "filtersConfig": content_filters }),
        )],
    );
    with(
        "a word policy of words only",
        "word policy words",
        VISIBILITY_SAVED,
        vec![("wordPolicyConfig", word_policy_words.clone())],
    );
    with(
        "a word policy of managed word lists only",
        "word policy managed",
        VISIBILITY_SAVED,
        vec![("wordPolicyConfig", word_policy_managed.clone())],
    );
    with(
        "a word policy of both words and managed word lists",
        "word policy both",
        VISIBILITY_PUBLISHED,
        vec![(
            "wordPolicyConfig",
            json!({
                "wordsConfig": [{ "text": "forbidden" }],
                "managedWordListsConfig": [{ "type": MANAGED_WORDS_PROFANITY }],
            }),
        )],
    );
    with(
        "a sensitive information policy of PII entities only",
        "sensitive pii",
        VISIBILITY_SAVED,
        vec![(
            "sensitiveInformationPolicyConfig",
            json!({ "piiEntitiesConfig": pii_entities }),
        )],
    );
    with(
        "a sensitive information policy of regular expressions only",
        "sensitive regexes",
        VISIBILITY_SAVED,
        vec![("sensitiveInformationPolicyConfig", sensitive_regexes.clone())],
    );
    with(
        "a contextual grounding policy with both filter types",
        "contextual grounding",
        VISIBILITY_PUBLISHED,
        vec![(
            "contextualGroundingPolicyConfig",
            json!({ "filtersConfig": grounding_filters }),
        )],
    );
    with(
        "several tags and no policy configuration at all",
        "tagged, unpoliced",
        VISIBILITY_SAVED,
        vec![("tags", json!(tag_map(3)))],
    );
    with(
        "one tag, a description and every policy configuration at once",
        "everything at once",
        VISIBILITY_PUBLISHED,
        vec![
            ("description", json!("Blocks everything this suite names.")),
            ("tags", json!(tag_map(1))),
            ("topicPolicyConfig", topic_policy),
            (
                "contentPolicyConfig",
                json!({ "filtersConfig": [{
                    "type": CONTENT_FILTER_TYPES[0],
                    "inputStrength": FILTER_STRENGTHS[3],
                    "outputStrength": FILTER_STRENGTHS[2],
                }] }),
            ),
            (
                "wordPolicyConfig",
                json!({
                    "wordsConfig": [{ "text": "forbidden" }],
                    "managedWordListsConfig": [{ "type": MANAGED_WORDS_PROFANITY }],
                }),
            ),
            (
                "sensitiveInformationPolicyConfig",
                json!({
                    "piiEntitiesConfig": [{ "type": PII_ENTITY_TYPES[0], "action": ACTIONS[0] }],
                    "regexesConfig": [{
                        "name": "employee id",
                        "pattern": "EMP-[0-9]{6}",
                        "action": ACTIONS[1],
                    }],
                }),
            ),
            (
                "contextualGroundingPolicyConfig",
                json!({ "filtersConfig": [{
                    "type": GROUNDING_TYPES[0],
                    "threshold": THRESHOLD_ABOVE_MIN,
                }] }),
            ),
        ],
    );

    cases
}

/// Every member the request carried is reported back unchanged, whichever answer is
/// being read — the create's own or a later read's.
pub(crate) fn assert_reports_request(case: &str, what: &str, body: &Value, reply: &Reply) {
    let members = body
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the request body must be an object"));
    for (member, sent) in members {
        if member == "clientToken" {
            continue;
        }
        let got = reply.field(&format!("/aiGuardrail/{member}"));
        assert_eq!(
            got,
            Some(sent),
            "case {case:?}: {what} must report the {member:?} the request carried, expected \
             {sent}, got {got:?}"
        );
    }
}

/// Whether `value` has the form the modelled `Uuid` shape declares
/// (`smithy_query(query="shape",shape="Uuid")`:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, expected)| {
                group.len() == expected
                    && group.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && group.bytes().all(|byte| !byte.is_ascii_uppercase())
            })
}

/// The ARN AWS documents for an AI guardrail of this service, written out here rather
/// than taken from the handler's helper so the test pins the format the modelled `Arn`
/// pattern describes: the guardrail's own id LAST, behind its parent assistant's.
pub(crate) fn expected_guardrail_arn(assistant_id: &str, guardrail_id: &str) -> String {
    wisdom_arn(
        "aws",
        REGION,
        engine::EMULATOR_ACCOUNT_ID,
        &format!("ai-guardrail/{assistant_id}/{guardrail_id}"),
    )
}

/// The ARN a read of one VERSION of a guardrail reports: the guardrail's own ARN with the
/// qualifier of the version read appended. An operation addressed through a label that may
/// carry a version qualifier names the version it answered for; the create, which
/// addresses no version, reports the bare resource ARN.
pub(crate) fn expected_read_arn(assistant_id: &str, guardrail_id: &str, qualifier: &str) -> String {
    format!(
        "{}:{qualifier}",
        expected_guardrail_arn(assistant_id, guardrail_id)
    )
}

#[tokio::test]
async fn a_created_guardrail_is_reported_out_of_the_stored_record_by_every_later_read() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "guardrail round trip").await;

    // Everything each row's create answered, so the reads below can be compared against
    // it rather than against the request alone.
    let mut created: Vec<(Case, String, Value)> = Vec::new();
    for case in cases() {
        let reply = create_guardrail(&emulator, &assistant, case.body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: the create must be served, got HTTP {} with body {}",
            case.label, reply.status, reply.body
        );
        assert_reports_request(&case.label, "the create", &case.body, &reply);

        let guardrail_id = reply.string("/aiGuardrail/aiGuardrailId");
        assert!(
            is_uuid_shaped(&guardrail_id),
            "case {:?}: the minted aiGuardrailId must have the modelled Uuid form, got \
             {guardrail_id:?}",
            case.label
        );
        let guardrail_arn = reply.string("/aiGuardrail/aiGuardrailArn");
        assert_eq!(
            guardrail_arn,
            expected_guardrail_arn(&assistant, &guardrail_id),
            "case {:?}: the guardrail must report the ARN of this account's guardrail in \
             {REGION}, got {guardrail_arn:?}",
            case.label
        );
        assert_eq!(
            reply.string("/aiGuardrail/assistantId"),
            assistant,
            "case {:?}: the guardrail must report the assistant the route named, body {}",
            case.label,
            reply.body
        );
        assert_eq!(
            reply.string("/aiGuardrail/assistantArn"),
            wisdom_arn(
                "aws",
                REGION,
                engine::EMULATOR_ACCOUNT_ID,
                &format!("assistant/{assistant}"),
            ),
            "case {:?}: the guardrail must report its parent assistant's own ARN, body {}",
            case.label,
            reply.body
        );
        assert_eq!(
            reply.string("/aiGuardrail/status"),
            STATUS_ACTIVE,
            "case {:?}: a created guardrail is in service, body {}",
            case.label,
            reply.body
        );
        let modified = reply.field("/aiGuardrail/modifiedTime").cloned();
        assert!(
            modified.as_ref().is_some_and(Value::is_number),
            "case {:?}: the guardrail must report when it was last modified, got \
             {modified:?}",
            case.label
        );
        // The `tags` member travels for every resource this service reports, as an empty
        // object when nothing tagged it.
        let tags = reply.field("/aiGuardrail/tags").cloned();
        assert!(
            tags.as_ref().is_some_and(Value::is_object),
            "case {:?}: the guardrail must report a tags map, got {tags:?}",
            case.label
        );

        let data = reply
            .field("/aiGuardrail")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: the create answered no aiGuardrail", case.label));
        created.push((case, guardrail_id, data));
    }

    // A read by id, by ARN, and by either carrying the working copy's version qualifier,
    // answer with the record the create stored.
    for (case, guardrail_id, data) in &created {
        for addressed_by in [
            guardrail_id.clone(),
            expected_guardrail_arn(&assistant, guardrail_id),
            format!("{guardrail_id}:{QUALIFIER_LATEST}"),
            expected_read_arn(&assistant, guardrail_id, QUALIFIER_LATEST),
        ] {
            let reply = get_guardrail(&emulator, &assistant, &addressed_by).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: a read addressed by {addressed_by:?} must be served, got HTTP {} \
                 with body {}",
                case.label, reply.status, reply.body
            );
            assert_reads_the_create(&case.label, &addressed_by, data, &reply);
            assert_reports_request(&case.label, "a later read", &case.body, &reply);
            // The model: the version number is reported only when the request's
            // `aiGuardrailId` carried a version qualifier, which this one did not.
            assert!(
                reply.field("/versionNumber").is_none(),
                "case {:?}: a read of the guardrail itself reports no versionNumber, body {}",
                case.label,
                reply.body
            );
        }
    }

    // A second read of an unchanged guardrail answers alike: reading one changes nothing.
    for (case, guardrail_id, data) in &created {
        let reply = get_guardrail(&emulator, &assistant, guardrail_id).await;
        assert_reads_the_create(&case.label, "a repeated read", data, &reply);
    }

    // The delete forgets just that record: the guardrail it named is no longer found,
    // and every guardrail still held reads back unchanged.
    let mut remaining: Vec<(Case, String, Value)> = created;
    while let Some((case, guardrail_id, _)) = remaining.pop() {
        let reply = delete_guardrail(&emulator, &assistant, &guardrail_id).await;
        assert_eq!(
            reply.status, 202,
            "case {:?}: the delete must be served, got HTTP {} with body {}",
            case.label, reply.status, reply.body
        );
        let reply = get_guardrail(&emulator, &assistant, &guardrail_id).await;
        assert_not_found(
            &case.label,
            MESSAGE_AI_GUARDRAIL_NOT_FOUND,
            &guardrail_id,
            &reply,
        );
        for (other, other_id, data) in &remaining {
            let reply = get_guardrail(&emulator, &assistant, other_id).await;
            assert_reads_the_create(&other.label, "a read after another delete", data, &reply);
        }
    }
}

/// A read answers with the `AIGuardrailData` the create stored, member for member — with
/// the one member a read renders differently: the ARN it reports names the version it
/// answered for, which for the guardrail's working copy is the floating alias.
pub(crate) fn assert_reads_the_create(case: &str, what: &str, created: &Value, reply: &Reply) {
    assert_eq!(reply.status, 200, "case {case:?}: {what} must be served, got HTTP {} with body {}", reply.status, reply.body);
    let mut expected = created.clone();
    let stored_arn = expected
        .get("aiGuardrailArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    expected["aiGuardrailArn"] = json!(format!("{stored_arn}:{QUALIFIER_LATEST}"));
    let read = reply.field("/aiGuardrail");
    assert_eq!(
        read,
        Some(&expected),
        "case {case:?}: {what} must answer with the record the create stored, expected \
         {expected}, got {read:?}"
    );
}

/// The modelled `ResourceNotFoundException` (HTTP 404), carrying this endpoint's fixed
/// sentence for the condition and the label the request used.
pub(crate) fn assert_not_found(case: &str, message: &str, resource_name: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: a label naming nothing this account holds is not found, got HTTP {} \
         with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ResourceNotFoundException",
        "case {case:?}: the refusal must carry the modelled error code, got {:?} with body {}",
        reply.error_type, reply.body
    );
    assert_eq!(
        reply.string("/message"),
        message,
        "case {case:?}: the refusal must carry this endpoint's sentence for the condition, \
         body {}",
        reply.body
    );
    assert_eq!(
        reply.string("/resourceName"),
        resource_name,
        "case {case:?}: the refusal must name the label the request used, body {}",
        reply.body
    );
}

#[tokio::test]
async fn a_guardrail_name_identifies_it_inside_its_assistant() {
    let emulator = Emulator::new();
    let first = assistant_id(&emulator, "name conflict first").await;
    let second = assistant_id(&emulator, "name conflict second").await;

    // Rows differing in the form of the name, each asserted the same way.
    for name in [
        "Blocks everything",
        &"n".repeat(NAME_MAX),
        "under_scores.and,punctuation-1",
    ] {
        let created = create_guardrail(&emulator, &first, required(name, VISIBILITY_SAVED)).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the first create must be served, got HTTP {} with body {}",
            created.status, created.body
        );
        let guardrail_id = created.string("/aiGuardrail/aiGuardrailId");

        // A second create under the same name in the same assistant is the conflict,
        // and it leaves the first record exactly as it was.
        let conflict =
            create_guardrail(&emulator, &first, required(name, VISIBILITY_PUBLISHED)).await;
        assert_eq!(
            conflict.status, 409,
            "case {name:?}: a second create under a live name is refused, got HTTP {} with \
             body {}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_type, "ConflictException",
            "case {name:?}: the refusal must carry the modelled error code, got {:?}",
            conflict.error_type
        );
        assert_eq!(
            conflict.string("/message"),
            MESSAGE_NAME_IN_USE,
            "case {name:?}: the refusal must carry this endpoint's sentence for the \
             condition, body {}",
            conflict.body
        );
        let read = get_guardrail(&emulator, &first, &guardrail_id).await;
        assert_reads_the_create(
            name,
            "a read after the refused create",
            created
                .field("/aiGuardrail")
                .expect("the create answered an aiGuardrail"),
            &read,
        );

        // Another assistant may hold the same name: a guardrail is scoped to its parent.
        let elsewhere = create_guardrail(&emulator, &second, required(name, VISIBILITY_SAVED)).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {name:?}: another assistant may hold the same name, got HTTP {} with body \
             {}",
            elsewhere.status, elsewhere.body
        );
        let elsewhere_id = elsewhere.string("/aiGuardrail/aiGuardrailId");
        assert_ne!(
            elsewhere_id, guardrail_id,
            "case {name:?}: two assistants' guardrails are two records, got one id \
             {guardrail_id:?}"
        );
        // And one assistant's guardrail is not the other's.
        let crossed = get_guardrail(&emulator, &second, &guardrail_id).await;
        assert_not_found(
            name,
            MESSAGE_AI_GUARDRAIL_NOT_FOUND,
            &guardrail_id,
            &crossed,
        );

        // The name the record held is released by its delete, so a create may take it
        // again.
        let deleted = delete_guardrail(&emulator, &first, &guardrail_id).await;
        assert_eq!(
            deleted.status, 202,
            "case {name:?}: the delete must be served, got HTTP {} with body {}",
            deleted.status, deleted.body
        );
        let again = create_guardrail(&emulator, &first, required(name, VISIBILITY_SAVED)).await;
        assert_eq!(
            again.status, 200,
            "case {name:?}: the name a delete released may be taken again, got HTTP {} with \
             body {}",
            again.status, again.body
        );
        // Leave the assistants empty for the next row.
        for (parent, id) in [
            (&first, again.string("/aiGuardrail/aiGuardrailId")),
            (&second, elsewhere_id),
        ] {
            let reply = delete_guardrail(&emulator, parent, &id).await;
            assert_eq!(
                reply.status, 202,
                "case {name:?}: the cleanup delete must be served, got HTTP {} with body {}",
                reply.status, reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_replayed_client_token_answers_with_the_guardrail_it_already_created() {
    let emulator = Emulator::new();
    let first = assistant_id(&emulator, "idempotent first").await;
    let second = assistant_id(&emulator, "idempotent second").await;

    // Rows: the token's form, and whether the replayed request carries the same members.
    for token in ["a-client-token", &"t".repeat(4096), "1"] {
        let mut body = required("idempotent guardrail", VISIBILITY_SAVED);
        body["clientToken"] = json!(token);

        let created = create_guardrail(&emulator, &first, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {token:?}: the create must be served, got HTTP {} with body {}",
            created.status, created.body
        );
        let guardrail_id = created.string("/aiGuardrail/aiGuardrailId");

        let replayed = create_guardrail(&emulator, &first, body.clone()).await;
        assert_eq!(
            replayed.field("/aiGuardrail"),
            created.field("/aiGuardrail"),
            "case {token:?}: a replayed token answers with the guardrail it already created"
        );

        // The same token carrying a DIFFERENT request is the idempotency conflict.
        let mut changed = body.clone();
        changed["blockedInputMessaging"] = json!("Something else entirely.");
        let conflict = create_guardrail(&emulator, &first, changed).await;
        assert_eq!(
            conflict.status, 409,
            "case {token:?}: one token identifies one request, got HTTP {} with body {}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.string("/message"),
            MESSAGE_IDEMPOTENCY_CONFLICT,
            "case {token:?}: the refusal must carry this endpoint's sentence for the \
             condition, body {}",
            conflict.body
        );

        // The token is scoped to the assistant it created under, so the same token on
        // another assistant creates another guardrail.
        let elsewhere = create_guardrail(&emulator, &second, body).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {token:?}: the same token on another parent creates another guardrail, got \
             HTTP {} with body {}",
            elsewhere.status, elsewhere.body
        );
        let elsewhere_id = elsewhere.string("/aiGuardrail/aiGuardrailId");
        assert_ne!(
            elsewhere_id, guardrail_id,
            "case {token:?}: the two creates must be two records, got one id {guardrail_id:?}"
        );

        for (parent, id) in [(&first, &guardrail_id), (&second, &elsewhere_id)] {
            let reply = delete_guardrail(&emulator, parent, id).await;
            assert_eq!(
                reply.status, 202,
                "case {token:?}: the cleanup delete must be served, got HTTP {} with body {}",
                reply.status, reply.body
            );
        }
        assert_eq!(
            replayed.status, 200,
            "case {token:?}: the replay itself was served, got HTTP {}",
            replayed.status
        );
    }
}

#[tokio::test]
async fn a_label_naming_no_guardrail_of_this_assistant_is_not_found() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "guardrail not found").await;
    let other = assistant_id(&emulator, "guardrail not found elsewhere").await;

    let held = create_guardrail(&emulator, &assistant, required("held", VISIBILITY_SAVED)).await;
    let held_id = held.string("/aiGuardrail/aiGuardrailId");
    let elsewhere =
        create_guardrail(&emulator, &other, required("elsewhere", VISIBILITY_SAVED)).await;
    let elsewhere_id = elsewhere.string("/aiGuardrail/aiGuardrailId");
    let gone = create_guardrail(&emulator, &assistant, required("gone", VISIBILITY_SAVED)).await;
    let gone_id = gone.string("/aiGuardrail/aiGuardrailArn");
    assert_eq!(
        delete_guardrail(&emulator, &assistant, &gone_id).await.status,
        202,
        "the record this row reads after its delete must be deleted first"
    );

    // Rows: the id form, the parent it belongs to, the ARN's account and partition, a
    // version qualifier, and a record that was deleted. (The ARN's REGION is not a row:
    // this service reads a label's id out of an ARN of this partition and account
    // whatever region it names, for every resource family alike, so a cross-region ARN is
    // not a refusal of this operation.)
    let rows: Vec<(&str, String)> = vec![
        ("a well-formed id this assistant holds nothing under", ABSENT_UUID.to_string()),
        (
            "an id another assistant's guardrail holds",
            elsewhere_id.clone(),
        ),
        ("a guardrail this assistant deleted", gone_id.clone()),
        (
            "a live id carrying a numeric version qualifier",
            format!("{held_id}:1"),
        ),
        (
            "a live ARN carrying a version qualifier",
            format!("{}:1", expected_guardrail_arn(&assistant, &held_id)),
        ),
        (
            "an ARN of this account naming nothing",
            expected_guardrail_arn(&assistant, ABSENT_UUID),
        ),
        (
            "a live id under another account's ARN",
            wisdom_arn(
                "aws",
                REGION,
                OTHER_ACCOUNT,
                &format!("ai-guardrail/{assistant}/{held_id}"),
            ),
        ),
        (
            "a live id under another partition's ARN",
            wisdom_arn(
                "aws-cn",
                REGION,
                engine::EMULATOR_ACCOUNT_ID,
                &format!("ai-guardrail/{assistant}/{held_id}"),
            ),
        ),
    ];
    for (case, label_value) in &rows {
        let read = get_guardrail(&emulator, &assistant, label_value).await;
        assert_not_found(case, MESSAGE_AI_GUARDRAIL_NOT_FOUND, label_value, &read);
        let deleted = delete_guardrail(&emulator, &assistant, label_value).await;
        assert_not_found(case, MESSAGE_AI_GUARDRAIL_NOT_FOUND, label_value, &deleted);
    }

    // The guardrail every row above left alone is still there, read out of its record.
    let read = get_guardrail(&emulator, &assistant, &held_id).await;
    assert_reads_the_create(
        "the refusals leave the stored guardrail untouched",
        "a read after every refusal",
        held.field("/aiGuardrail")
            .expect("the create answered an aiGuardrail"),
        &read,
    );

    // A request addressed to an assistant this account does not hold is not found,
    // whatever the rest of the route names — on the create too.
    for (case, assistant_label) in [
        ("a well-formed assistant id nothing holds", ABSENT_UUID.to_string()),
        (
            "an assistant ARN of another account",
            wisdom_arn(
                "aws",
                REGION,
                OTHER_ACCOUNT,
                &format!("assistant/{assistant}"),
            ),
        ),
    ] {
        let created = create_guardrail(
            &emulator,
            &assistant_label,
            required("under no assistant", VISIBILITY_SAVED),
        )
        .await;
        assert_not_found(case, MESSAGE_ASSISTANT_NOT_FOUND, &assistant_label, &created);
        let read = get_guardrail(&emulator, &assistant_label, &held_id).await;
        assert_not_found(case, MESSAGE_ASSISTANT_NOT_FOUND, &assistant_label, &read);
        let deleted = delete_guardrail(&emulator, &assistant_label, &held_id).await;
        assert_not_found(case, MESSAGE_ASSISTANT_NOT_FOUND, &assistant_label, &deleted);
    }

    // A guardrail is reachable only through its assistant, so the assistant's delete
    // takes it with it.
    assert_eq!(
        emulator.delete_assistant(&assistant).await.status,
        204,
        "the assistant delete must be served"
    );
    let orphaned = get_guardrail(&emulator, &assistant, &held_id).await;
    assert_not_found(
        "the guardrail of a deleted assistant",
        MESSAGE_ASSISTANT_NOT_FOUND,
        &assistant,
        &orphaned,
    );
}

#[tokio::test]
async fn the_tags_a_create_persisted_on_a_guardrail_are_what_list_tags_reports() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "guardrail tags").await;

    // Rows: the tag count — none, one, several.
    for count in [0_usize, 1, 7] {
        let tags = tag_map(count);
        let mut body = required(&format!("tagged with {count}"), VISIBILITY_SAVED);
        if count > 0 {
            body["tags"] = json!(tags);
        }
        let created = create_guardrail(&emulator, &assistant, body).await;
        assert_eq!(
            created.status, 200,
            "case {count}: the create must be served, got HTTP {} with body {}",
            created.status, created.body
        );
        let guardrail_arn = created.string("/aiGuardrail/aiGuardrailArn");

        let reported = emulator.list_tags(&guardrail_arn).await;
        assert_eq!(
            reported.status, 200,
            "case {count}: the guardrail's ARN must name a resource the tagging surface \
             holds, got HTTP {} with body {}",
            reported.status, reported.body
        );
        assert_eq!(
            reported.field("/tags"),
            created.field("/aiGuardrail/tags"),
            "case {count}: the reported map must be the one the create persisted"
        );
        assert_eq!(
            reported.field("/tags").cloned().unwrap_or(Value::Null),
            json!(tags),
            "case {count}: the reported map must be the one the request carried"
        );

        // The record a delete removed holds no tags for the tagging surface to report.
        let guardrail_id = created.string("/aiGuardrail/aiGuardrailId");
        assert_eq!(
            delete_guardrail(&emulator, &assistant, &guardrail_id)
                .await
                .status,
            202,
            "case {count}: the delete must be served"
        );
        let after = emulator.list_tags(&guardrail_arn).await;
        assert_eq!(
            after.status, 404,
            "case {count}: a deleted guardrail's ARN names no resource, got HTTP {} with \
             body {}",
            after.status, after.body
        );
    }
}

#[tokio::test]
async fn a_guardrail_policy_is_bounded_by_the_stores_entry_ceiling() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "guardrail policy ceiling").await;

    // A policy whose entries are spread across every one of the five configurations, so
    // the ceiling is asserted on the whole stored policy rather than on one list: the
    // fixed twelve below plus as many blocked words as the row asks for. Words are what
    // the bulk is made of because they are the list AWS documents the largest maximum for
    // (ten thousand), so the heap guard has to sit above it.
    let policy = |words: usize| -> Vec<(&'static str, Value)> {
        let words_config: Vec<Value> = (0..words)
            .map(|index| json!({ "text": format!("word {index}") }))
            .collect();
        let filters_config: Vec<Value> = CONTENT_FILTER_TYPES
            .iter()
            .map(|filter| {
                json!({
                    "type": filter,
                    "inputStrength": FILTER_STRENGTHS[1],
                    "outputStrength": FILTER_STRENGTHS[1],
                })
            })
            .collect();
        vec![
            (
                "topicPolicyConfig",
                json!({ "topicsConfig": [
                    {
                        "name": "Investment advice",
                        "definition": "A topic this suite denies.",
                        "type": TOPIC_TYPE_DENY,
                    },
                    {
                        "name": "Legal advice",
                        "definition": "Another topic this suite denies.",
                        "type": TOPIC_TYPE_DENY,
                    },
                ] }),
            ),
            (
                "contentPolicyConfig",
                json!({ "filtersConfig": filters_config }),
            ),
            (
                "wordPolicyConfig",
                json!({
                    "wordsConfig": words_config,
                    "managedWordListsConfig": [{ "type": MANAGED_WORDS_PROFANITY }],
                }),
            ),
            (
                "sensitiveInformationPolicyConfig",
                json!({
                    "piiEntitiesConfig": [{ "type": PII_ENTITY_TYPES[0], "action": ACTIONS[0] }],
                    "regexesConfig": [{
                        "name": "employee id",
                        "pattern": "EMP-[0-9]{6}",
                        "action": ACTIONS[0],
                    }],
                }),
            ),
            (
                "contextualGroundingPolicyConfig",
                json!({ "filtersConfig": [{
                    "type": GROUNDING_TYPES[0],
                    "threshold": THRESHOLD_MIN,
                }] }),
            ),
        ]
    };
    /// The entries the five configurations above carry besides the blocked words: two
    /// denied topics, six content filters, a managed word list, a PII entity and a
    /// regular expression, and one grounding filter.
    const FIXED_ENTRIES: usize = 12;

    let body = |name: &str, words: usize| {
        let mut body = required(name, VISIBILITY_SAVED);
        for (member, value) in policy(words) {
            body[member] = value;
        }
        body
    };

    // Exactly at the ceiling: stored, and read back out of the record.
    let at_ceiling = create_guardrail(
        &emulator,
        &assistant,
        body(
            "policy at the ceiling",
            MAX_AI_GUARDRAIL_POLICY_ENTRIES - FIXED_ENTRIES,
        ),
    )
    .await;
    assert_eq!(
        at_ceiling.status, 200,
        "a policy of exactly {MAX_AI_GUARDRAIL_POLICY_ENTRIES} entries must be stored, got \
         HTTP {} with body {}",
        at_ceiling.status, at_ceiling.body
    );
    let guardrail_id = at_ceiling.string("/aiGuardrail/aiGuardrailId");
    let read = get_guardrail(&emulator, &assistant, &guardrail_id).await;
    assert_reads_the_create(
        "a policy at the ceiling",
        "a read of the stored policy",
        at_ceiling
            .field("/aiGuardrail")
            .expect("the create answered an aiGuardrail"),
        &read,
    );

    // One entry past it: refused, and nothing stored under the name it asked for.
    let refused = create_guardrail(
        &emulator,
        &assistant,
        body(
            "policy past the ceiling",
            MAX_AI_GUARDRAIL_POLICY_ENTRIES - FIXED_ENTRIES + 1,
        ),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "a policy past the ceiling is refused, got HTTP {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "ValidationException",
        "the refusal must carry the modelled error code, got {:?}",
        refused.error_type
    );
    assert!(
        refused
            .string("/message")
            .contains(&MAX_AI_GUARDRAIL_POLICY_ENTRIES.to_string()),
        "the refusal must state the ceiling it enforces, body {}",
        refused.body
    );
    let smaller = create_guardrail(
        &emulator,
        &assistant,
        body("policy past the ceiling", FIXED_ENTRIES),
    )
    .await;
    assert_eq!(
        smaller.status, 200,
        "the refused create must have stored nothing, so its name is still free, got HTTP \
         {} with body {}",
        smaller.status, smaller.body
    );
}

#[tokio::test]
async fn an_assistant_holds_at_most_the_stores_guardrail_ceiling() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "guardrail ceiling").await;

    for index in 0..MAX_AI_GUARDRAILS_PER_ASSISTANT {
        let reply = create_guardrail(
            &emulator,
            &assistant,
            required(&format!("ceiling {index}"), VISIBILITY_SAVED),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {index}: a create inside the ceiling must be served, got HTTP {} with body \
             {}",
            reply.status, reply.body
        );
    }
    let refused = create_guardrail(
        &emulator,
        &assistant,
        required("one too many", VISIBILITY_SAVED),
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "a create past the ceiling is the modelled service-quota refusal, got HTTP {} with \
         body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "ServiceQuotaExceededException",
        "the refusal must carry the modelled error code, got {:?}",
        refused.error_type
    );
    assert!(
        refused
            .string("/message")
            .contains(&MAX_AI_GUARDRAILS_PER_ASSISTANT.to_string()),
        "the refusal must state the ceiling it enforces, body {}",
        refused.body
    );
}
