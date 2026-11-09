// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateAIGuardrail` and the `GetAIGuardrail` read that must serve
//! what it wrote, driven over HTTP through the same router the emulator serves with.
//!
//! Two rules are under test, each asserted over a TABLE of inputs rather than the one
//! recorded request:
//!
//! * an update WRITES the working copy of the AI guardrail the route names — the members
//!   the request requires and every optional member it carries replace the stored values, a
//!   member it carries none of keeps the value it had, and the members the SERVICE owns
//!   (the id, the parentage, the name, the status and the tags) are untouched — and every
//!   later read answers out of that written record rather than out of the creation
//!   snapshot;
//! * a READ of a guardrail reports the ARN of the version it answered for, which for the
//!   working copy is the floating `$LATEST` alias; a create or an update, which address the
//!   resource rather than one of its versions, report the bare resource ARN.
//!
//! The rows differ along every axis the rules generalise over:
//!
//! * both values the modelled `VisibilityStatus` declares, in both directions;
//! * both blocked-messaging members at both published length bounds;
//! * `description` at both published bounds, and omitted — which keeps the stored one;
//! * each of the five policy configurations, added by the update, replaced by the update
//!   and left for the update to keep, with the content policy spanning the filter-type and
//!   strength catalogs;
//! * one update, and several updates layered on one record;
//! * how the record is addressed afterwards: by id, by ARN, and by either carrying the
//!   working copy's version qualifier.
//!
//! Beside the write it pins the refusals: a parent assistant this account does not hold, a
//! guardrail its assistant does not hold, a guardrail of ANOTHER assistant, a numbered
//! version qualifier (a published snapshot is not the working copy an update writes), a
//! resulting policy past the store's entry ceiling, and a replayed `clientToken` — answered
//! with the record that token's own write left, or refused when the request behind the token
//! differs. After every refusal the stored record is read back unchanged.
//!
//! Every assertion goes over the wire — only the HTTP status, the `x-amzn-errortype` header
//! and the JSON body are inspected — so a response that is not computed from persisted
//! state cannot pass. The harness (the router, the reply reader, the assistant create and
//! the guardrail create, read and delete) is the one `resource_tags.rs` and
//! `ai_guardrail.rs` already drive this service with.

use serde_json::{json, Value};

use service_com_amazonaws_qconnect_wisdomservice::state::MAX_AI_GUARDRAIL_POLICY_ENTRIES;

use super::ai_guardrail::{
    assert_not_found, assistant_id, create_guardrail, delete_guardrail, expected_guardrail_arn,
    expected_read_arn, get_guardrail, required, ACTIONS, CONTENT_FILTER_TYPES, DESCRIPTION_MAX,
    DESCRIPTION_MIN, FILTER_STRENGTHS, GROUNDING_TYPES, MANAGED_WORDS_PROFANITY, MESSAGING_MAX,
    MESSAGING_MIN, MESSAGE_AI_GUARDRAIL_NOT_FOUND, MESSAGE_ASSISTANT_NOT_FOUND,
    MESSAGE_IDEMPOTENCY_CONFLICT, PII_ENTITY_TYPES, QUALIFIER_LATEST, THRESHOLD_ABOVE_MIN,
    THRESHOLD_MIN, TOPIC_TYPE_DENY, VISIBILITY_PUBLISHED, VISIBILITY_SAVED,
};
use super::resource_tags::{label, tag_map, wisdom_arn, Emulator, Reply, ABSENT_UUID, OTHER_ACCOUNT, REGION};

/// The members of `AIGuardrailData` an update may write
/// (`smithy_query(query="operation",operation="UpdateAIGuardrail")`: `visibilityStatus`,
/// `blockedInputMessaging` and `blockedOutputsMessaging` are required, `description` and
/// the five policy configurations optional).
const WRITABLE_MEMBERS: [&str; 9] = [
    "visibilityStatus",
    "blockedInputMessaging",
    "blockedOutputsMessaging",
    "description",
    "topicPolicyConfig",
    "contentPolicyConfig",
    "wordPolicyConfig",
    "sensitiveInformationPolicyConfig",
    "contextualGroundingPolicyConfig",
];

/// The members of `AIGuardrailData` no update may change: the guardrail's identity and
/// parentage, the name it was created with, and the members the SERVICE owns
/// (`smithy_query(query="shape",shape="AIGuardrailData")`).
const PRESERVED_MEMBERS: [&str; 6] = [
    "aiGuardrailId",
    "aiGuardrailArn",
    "assistantId",
    "assistantArn",
    "name",
    "status",
];

/// `UpdateAIGuardrail` on one guardrail, as the model's URI binds it
/// (`POST /assistants/{assistantId}/aiguardrails/{aiGuardrailId}`).
async fn update_guardrail(
    emulator: &Emulator,
    assistant: &str,
    guardrail: &str,
    body: Value,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!(
                "/assistants/{}/aiguardrails/{}",
                label(assistant),
                label(guardrail)
            ),
            Some(body),
        )
        .await
}

/// The members every update in this suite is required to carry, so a row states only what
/// it varies.
fn required_update(visibility: &str) -> Value {
    json!({
        "visibilityStatus": visibility,
        "blockedInputMessaging": "The update's blocked-input message.",
        "blockedOutputsMessaging": "The update's blocked-output message.",
    })
}

/// A `topicPolicyConfig` body, varying the entry count so two rows differ.
fn topic_policy(entries: usize) -> Value {
    let topics: Vec<Value> = (0..entries)
        .map(|index| {
            json!({
                "name": format!("Denied topic {index}"),
                "definition": format!("The topic this row denies, number {index}."),
                "type": TOPIC_TYPE_DENY,
            })
        })
        .collect();
    json!({ "topicsConfig": topics })
}

/// A `contentPolicyConfig` body naming the first `filters` entries of the modelled filter
/// catalog, each at a strength of the modelled strength catalog.
fn content_policy(filters: usize) -> Value {
    let filters_config: Vec<Value> = CONTENT_FILTER_TYPES
        .iter()
        .take(filters)
        .enumerate()
        .map(|(index, filter)| {
            json!({
                "type": filter,
                "inputStrength": FILTER_STRENGTHS[index % FILTER_STRENGTHS.len()],
                "outputStrength": FILTER_STRENGTHS[(index + 1) % FILTER_STRENGTHS.len()],
            })
        })
        .collect();
    json!({ "filtersConfig": filters_config })
}

/// A `wordPolicyConfig` body carrying both the words and the managed lists.
fn word_policy(words: usize) -> Value {
    let words_config: Vec<Value> = (0..words)
        .map(|index| json!({ "text": format!("word-{index}") }))
        .collect();
    json!({
        "wordsConfig": words_config,
        "managedWordListsConfig": [{ "type": MANAGED_WORDS_PROFANITY }],
    })
}

/// A `sensitiveInformationPolicyConfig` body carrying one PII entity and one regular
/// expression, each with an action of the modelled action catalog.
fn sensitive_policy(index: usize) -> Value {
    json!({
        "piiEntitiesConfig": [{
            "type": PII_ENTITY_TYPES[index % PII_ENTITY_TYPES.len()],
            "action": ACTIONS[index % ACTIONS.len()],
        }],
        "regexesConfig": [{
            "name": format!("pattern {index}"),
            "pattern": format!("ID-{index}-[0-9]+"),
            "action": ACTIONS[(index + 1) % ACTIONS.len()],
        }],
    })
}

/// A `contextualGroundingPolicyConfig` body, one filter of the modelled type catalog at the
/// threshold given.
fn grounding_policy(index: usize, threshold: f64) -> Value {
    json!({ "filtersConfig": [{
        "type": GROUNDING_TYPES[index % GROUNDING_TYPES.len()],
        "threshold": threshold,
    }] })
}

/// One row of the update table: the guardrail it creates, and the update bodies it then
/// sends in order (one row may layer several updates over one record).
#[derive(Debug, Clone)]
struct Row {
    label: String,
    create: Value,
    updates: Vec<Value>,
}

/// The create and update bodies the write rule is asserted over. Each row differs from the
/// others along one axis of the rule — the visibility status it moves to, the length of the
/// members it replaces, which optional members the request carries, which policy
/// configuration it writes, and how many updates are layered on one record.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // A guardrail created with a description and every policy configuration, so a row can
    // assert both what an update replaces and what it keeps.
    let full_create = |name: &str, visibility: &str| {
        let mut body = required("name placeholder", visibility);
        body["name"] = json!(name);
        body["description"] = json!("The description the create stored.");
        body["topicPolicyConfig"] = topic_policy(1);
        body["contentPolicyConfig"] = content_policy(1);
        body["wordPolicyConfig"] = word_policy(1);
        body["sensitiveInformationPolicyConfig"] = sensitive_policy(0);
        body["contextualGroundingPolicyConfig"] = grounding_policy(0, THRESHOLD_MIN);
        body["tags"] = json!(tag_map(2));
        body
    };

    // Both directions of the visibility move, each with every policy configuration
    // replaced by a different one and the description rewritten.
    for (index, (from, to)) in [
        (VISIBILITY_SAVED, VISIBILITY_PUBLISHED),
        (VISIBILITY_PUBLISHED, VISIBILITY_SAVED),
    ]
    .into_iter()
    .enumerate()
    {
        let mut update = required_update(to);
        update["description"] = json!("The description the update wrote.");
        update["topicPolicyConfig"] = topic_policy(2);
        update["contentPolicyConfig"] = content_policy(CONTENT_FILTER_TYPES.len());
        update["wordPolicyConfig"] = word_policy(3);
        update["sensitiveInformationPolicyConfig"] = sensitive_policy(1);
        update["contextualGroundingPolicyConfig"] = grounding_policy(1, THRESHOLD_ABOVE_MIN);
        rows.push(Row {
            label: format!("every member replaced, {from} -> {to}"),
            create: full_create(&format!("replace all {index}"), from),
            updates: vec![update],
        });
    }

    // Only the required members: the stored description and every stored policy are what
    // the record keeps.
    rows.push(Row {
        label: "the required members alone".to_string(),
        create: full_create("required members alone", VISIBILITY_SAVED),
        updates: vec![required_update(VISIBILITY_PUBLISHED)],
    });

    // The blocked messaging at each published bound of its length.
    for (bound, length) in [("minimum", MESSAGING_MIN), ("maximum", MESSAGING_MAX)] {
        let mut update = required_update(VISIBILITY_PUBLISHED);
        update["blockedInputMessaging"] = json!("b".repeat(length));
        update["blockedOutputsMessaging"] = json!("o".repeat(length));
        rows.push(Row {
            label: format!("blocked messaging at both published {bound} lengths ({length})"),
            create: full_create(&format!("messaging {bound}"), VISIBILITY_SAVED),
            updates: vec![update],
        });
    }

    // The description at each published bound of its length.
    for (bound, length) in [("minimum", DESCRIPTION_MIN), ("maximum", DESCRIPTION_MAX)] {
        let mut update = required_update(VISIBILITY_SAVED);
        update["description"] = json!("d".repeat(length));
        rows.push(Row {
            label: format!("description at its published {bound} length ({length})"),
            create: full_create(&format!("description {bound}"), VISIBILITY_PUBLISHED),
            updates: vec![update],
        });
    }

    // One policy configuration at a time, written onto a guardrail created with NONE of
    // them: the member the update carries is stored and the others stay absent.
    for (member, policy) in [
        ("topicPolicyConfig", topic_policy(2)),
        ("contentPolicyConfig", content_policy(2)),
        ("wordPolicyConfig", word_policy(2)),
        ("sensitiveInformationPolicyConfig", sensitive_policy(1)),
        (
            "contextualGroundingPolicyConfig",
            grounding_policy(1, THRESHOLD_ABOVE_MIN),
        ),
    ] {
        let mut update = required_update(VISIBILITY_PUBLISHED);
        update[member] = policy;
        rows.push(Row {
            label: format!("{member} added to a guardrail created without one"),
            create: required(&format!("added {member}"), VISIBILITY_SAVED),
            updates: vec![update],
        });
    }

    // An update restating exactly what is stored: the record is written, and reports the
    // same values.
    let mut restated = required_update(VISIBILITY_SAVED);
    restated["blockedInputMessaging"] = json!("This prompt was blocked.");
    restated["blockedOutputsMessaging"] = json!("This response was blocked.");
    rows.push(Row {
        label: "an update restating the stored values".to_string(),
        create: required("restated", VISIBILITY_SAVED),
        updates: vec![restated],
    });

    // Three updates layered on one record: each one is applied to the record the previous
    // left, and the last write is what the record holds.
    let mut second = required_update(VISIBILITY_SAVED);
    second["contentPolicyConfig"] = content_policy(CONTENT_FILTER_TYPES.len());
    let mut third = required_update(VISIBILITY_PUBLISHED);
    third["description"] = json!("The third update.");
    third["blockedInputMessaging"] = json!("b".repeat(MESSAGING_MAX));
    let mut first = required_update(VISIBILITY_PUBLISHED);
    first["description"] = json!("The first update.");
    rows.push(Row {
        label: "three updates layered on one record".to_string(),
        create: required("layered", VISIBILITY_SAVED),
        updates: vec![first, second, third],
    });

    rows
}

/// The `AIGuardrailData` an update must answer with: every member it carried replaces the
/// stored value, every other writable member is the one the previous answer reported, and
/// the members the service owns — the ARN of the resource among them — are untouched.
fn assert_applies_update(case: &str, what: &str, stored: &Value, update: &Value, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: {what} must be served, got HTTP {} with body {}",
        reply.status, reply.body
    );
    let got = reply.field("/aiGuardrail").unwrap_or_else(|| {
        panic!(
            "case {case:?}: {what} answered no aiGuardrail: {}",
            reply.body
        )
    });

    for member in WRITABLE_MEMBERS {
        let expected = update.get(member).or_else(|| stored.get(member));
        let actual = got.get(member);
        assert_eq!(
            actual, expected,
            "case {case:?}: {what} must report the {member:?} the update carried, or the \
             stored one when it carried none, expected {expected:?}, got {actual:?}"
        );
    }
    for member in PRESERVED_MEMBERS {
        let expected = stored.get(member);
        let actual = got.get(member);
        assert_eq!(
            actual, expected,
            "case {case:?}: {what} must leave {member:?} as it was, expected {expected:?}, \
             got {actual:?}"
        );
    }
    let arn = got.get("aiGuardrailArn").and_then(Value::as_str);
    assert!(
        arn.is_some_and(|arn| !arn.ends_with(&format!(":{QUALIFIER_LATEST}"))),
        "case {case:?}: {what} addresses the resource rather than one of its versions, so it \
         reports the bare resource ARN, got {arn:?}"
    );

    // The write is what `modifiedTime` reports, so it never moves backwards.
    let was = stored
        .get("modifiedTime")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let now = got.get("modifiedTime").and_then(Value::as_f64);
    assert!(
        now.is_some_and(|now| now >= was),
        "case {case:?}: {what} must report when the record was last written, expected at \
         least {was}, got {now:?}"
    );
}

/// The record the update left, read back through `GetAIGuardrail` however it is addressed.
/// A read answers with that record, with the one member a read renders differently: the ARN
/// it reports names the version it answered for.
async fn assert_reads_back(
    emulator: &Emulator,
    case: &str,
    assistant: &str,
    guardrail_id: &str,
    written: &Value,
) {
    let mut expected = written.clone();
    expected["aiGuardrailArn"] = json!(expected_read_arn(
        assistant,
        guardrail_id,
        QUALIFIER_LATEST
    ));
    for addressed_by in [
        guardrail_id.to_string(),
        expected_guardrail_arn(assistant, guardrail_id),
        format!("{guardrail_id}:{QUALIFIER_LATEST}"),
        expected_read_arn(assistant, guardrail_id, QUALIFIER_LATEST),
    ] {
        let reply = get_guardrail(emulator, assistant, &addressed_by).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a read addressed by {addressed_by:?} must be served, got HTTP {} \
             with body {}",
            reply.status, reply.body
        );
        let read = reply.field("/aiGuardrail");
        assert_eq!(
            read,
            Some(&expected),
            "case {case:?}: a read addressed by {addressed_by:?} must answer with the record \
             the update wrote, expected {expected}, got {read:?}"
        );
        // The model: a version number is reported only when the request's `aiGuardrailId`
        // named a VERSION. An update writes the working copy and publishes nothing, so a
        // read of it names no numbered version.
        assert!(
            reply.field("/versionNumber").is_none(),
            "case {case:?}: a read of the working copy reports no versionNumber, body {}",
            reply.body
        );
    }

    // An update publishes no version, so a numbered qualifier still names nothing.
    let numbered = format!("{guardrail_id}:1");
    let reply = get_guardrail(emulator, assistant, &numbered).await;
    assert_not_found(case, MESSAGE_AI_GUARDRAIL_NOT_FOUND, &numbered, &reply);
}

#[tokio::test]
async fn an_update_writes_the_guardrail_every_later_read_answers_with() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "guardrail update").await;

    for row in rows() {
        let created = create_guardrail(&emulator, &assistant, row.create.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the create must be served, got HTTP {} with body {}",
            row.label, created.status, created.body
        );
        let guardrail_id = created.string("/aiGuardrail/aiGuardrailId");
        // The create addresses no version, so it reports the bare resource ARN.
        let created_arn = created.string("/aiGuardrail/aiGuardrailArn");
        assert_eq!(
            created_arn,
            expected_guardrail_arn(&assistant, &guardrail_id),
            "case {:?}: the create must report the bare resource ARN, got {created_arn:?}",
            row.label
        );
        let read_arn = expected_read_arn(&assistant, &guardrail_id, QUALIFIER_LATEST);
        assert_ne!(
            created_arn, read_arn,
            "case {:?}: the bare resource ARN is not the version-qualified one a read \
             reports",
            row.label
        );

        let mut stored = created
            .field("/aiGuardrail")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: the create answered no aiGuardrail", row.label));
        for (index, update) in row.updates.iter().enumerate() {
            let what = format!("update {} of {}", index + 1, row.updates.len());
            let reply = update_guardrail(&emulator, &assistant, &guardrail_id, update.clone()).await;
            assert_applies_update(&row.label, &what, &stored, update, &reply);
            let written = reply
                .field("/aiGuardrail")
                .cloned()
                .unwrap_or_else(|| panic!("case {:?}: {what} answered no aiGuardrail", row.label));
            assert_reads_back(&emulator, &row.label, &assistant, &guardrail_id, &written).await;
            // The next update is applied to the record this one left.
            stored = written;
        }

        assert_eq!(
            delete_guardrail(&emulator, &assistant, &guardrail_id)
                .await
                .status,
            202,
            "case {:?}: the cleanup delete must be served",
            row.label
        );
    }
}

#[tokio::test]
async fn an_update_addressed_through_the_version_qualifier_writes_the_working_copy() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "guardrail update addressing").await;

    // Rows: every label form the modelled `UuidOrArnOrEitherWithQualifier` admits for the
    // working copy — the bare id, the resource ARN, and either carrying the floating
    // qualifier. Each writes the one record, so the next row sees the previous row's write.
    let created = create_guardrail(
        &emulator,
        &assistant,
        required("addressed every way", VISIBILITY_SAVED),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the create must be served, got HTTP {} with body {}",
        created.status, created.body
    );
    let guardrail_id = created.string("/aiGuardrail/aiGuardrailId");
    let mut stored = created
        .field("/aiGuardrail")
        .cloned()
        .expect("the create answered an aiGuardrail");

    for (index, addressed_by) in [
        guardrail_id.clone(),
        expected_guardrail_arn(&assistant, &guardrail_id),
        format!("{guardrail_id}:{QUALIFIER_LATEST}"),
        expected_read_arn(&assistant, &guardrail_id, QUALIFIER_LATEST),
    ]
    .into_iter()
    .enumerate()
    {
        let mut update = required_update(if index % 2 == 0 {
            VISIBILITY_PUBLISHED
        } else {
            VISIBILITY_SAVED
        });
        update["blockedInputMessaging"] = json!(format!("Blocked by write {index}."));
        update["description"] = json!(format!("Written through label form {index}."));
        let reply = update_guardrail(&emulator, &assistant, &addressed_by, update.clone()).await;
        assert_applies_update(
            &addressed_by,
            "an update addressed by this label form",
            &stored,
            &update,
            &reply,
        );
        stored = reply
            .field("/aiGuardrail")
            .cloned()
            .unwrap_or_else(|| panic!("case {addressed_by:?}: the update answered no aiGuardrail"));
        assert_reads_back(
            &emulator,
            &addressed_by,
            &assistant,
            &guardrail_id,
            &stored,
        )
        .await;
    }
}

#[tokio::test]
async fn an_update_naming_nothing_this_account_holds_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "guardrail update refusals").await;
    let other = assistant_id(&emulator, "guardrail update refusals elsewhere").await;

    let created =
        create_guardrail(&emulator, &assistant, required("held", VISIBILITY_SAVED)).await;
    let guardrail_id = created.string("/aiGuardrail/aiGuardrailId");
    let elsewhere = create_guardrail(&emulator, &other, required("elsewhere", VISIBILITY_SAVED)).await;
    let elsewhere_id = elsewhere.string("/aiGuardrail/aiGuardrailId");

    // One write that must take effect, so every refusal below is asserted against a record
    // a mutation already moved.
    let mut update = required_update(VISIBILITY_PUBLISHED);
    update["contentPolicyConfig"] = content_policy(1);
    let reply = update_guardrail(&emulator, &assistant, &guardrail_id, update.clone()).await;
    let stored = created
        .field("/aiGuardrail")
        .cloned()
        .expect("the create answered an aiGuardrail");
    assert_applies_update(
        "the write every refusal is asserted against",
        "the update",
        &stored,
        &update,
        &reply,
    );
    let written = reply
        .field("/aiGuardrail")
        .cloned()
        .expect("the update answered an aiGuardrail");

    // Rows: the guardrail label form that names nothing this assistant holds.
    let guardrail_rows: Vec<(&str, String)> = vec![
        (
            "a well-formed id this assistant holds nothing under",
            ABSENT_UUID.to_string(),
        ),
        (
            "an id another assistant's guardrail holds",
            elsewhere_id.clone(),
        ),
        (
            "a live id carrying a numeric version qualifier",
            format!("{guardrail_id}:1"),
        ),
        (
            "a live ARN carrying a numeric version qualifier",
            format!("{}:1", expected_guardrail_arn(&assistant, &guardrail_id)),
        ),
        (
            "a live id under another account's ARN",
            wisdom_arn(
                "aws",
                REGION,
                OTHER_ACCOUNT,
                &format!("ai-guardrail/{assistant}/{guardrail_id}"),
            ),
        ),
    ];
    for (case, label_value) in &guardrail_rows {
        let refused = update_guardrail(
            &emulator,
            &assistant,
            label_value,
            required_update(VISIBILITY_SAVED),
        )
        .await;
        assert_not_found(case, MESSAGE_AI_GUARDRAIL_NOT_FOUND, label_value, &refused);
    }

    // Rows: the assistant label form that names no assistant this account holds.
    for (case, assistant_label) in [
        (
            "a well-formed assistant id nothing holds",
            ABSENT_UUID.to_string(),
        ),
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
        let refused = update_guardrail(
            &emulator,
            &assistant_label,
            &guardrail_id,
            required_update(VISIBILITY_SAVED),
        )
        .await;
        assert_not_found(
            case,
            MESSAGE_ASSISTANT_NOT_FOUND,
            &assistant_label,
            &refused,
        );
    }

    // A resulting policy past the store's entry ceiling is refused, and writes nothing.
    let mut past_ceiling = required_update(VISIBILITY_SAVED);
    past_ceiling["wordPolicyConfig"] = word_policy(MAX_AI_GUARDRAIL_POLICY_ENTRIES + 1);
    let refused = update_guardrail(&emulator, &assistant, &guardrail_id, past_ceiling).await;
    assert_eq!(
        refused.status, 400,
        "a resulting policy past the ceiling is refused, got HTTP {} with body {}",
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

    // Every refusal above left the record the one write produced.
    assert_reads_back(
        &emulator,
        "the refusals leave the written record untouched",
        &assistant,
        &guardrail_id,
        &written,
    )
    .await;
}

#[tokio::test]
async fn a_replayed_client_token_answers_with_the_record_its_write_left() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "guardrail update idempotency").await;

    // Rows: the token's form, which the rule does not depend on.
    for token in ["an-update-token", &"t".repeat(4096), "1"] {
        let created = create_guardrail(
            &emulator,
            &assistant,
            required(&format!("idempotent {}", token.len()), VISIBILITY_SAVED),
        )
        .await;
        let guardrail_id = created.string("/aiGuardrail/aiGuardrailId");
        let stored = created
            .field("/aiGuardrail")
            .cloned()
            .expect("the create answered an aiGuardrail");

        let mut body = required_update(VISIBILITY_PUBLISHED);
        body["clientToken"] = json!(token);
        body["description"] = json!("Written once.");
        let reply = update_guardrail(&emulator, &assistant, &guardrail_id, body.clone()).await;
        assert_applies_update(token, "the update", &stored, &body, &reply);
        let written = reply
            .field("/aiGuardrail")
            .cloned()
            .unwrap_or_else(|| panic!("case {token:?}: the update answered no aiGuardrail"));

        // The same token carrying the same request answers with the record that write
        // left, and writes nothing a second time.
        let replayed = update_guardrail(&emulator, &assistant, &guardrail_id, body.clone()).await;
        assert_eq!(
            replayed.field("/aiGuardrail"),
            Some(&written),
            "case {token:?}: a replayed token answers with the record its write left, body {}",
            replayed.body
        );

        // The same token carrying a DIFFERENT request is the modelled idempotency
        // conflict, and leaves the record as it was.
        let mut changed = body.clone();
        changed["blockedInputMessaging"] = json!("Something else entirely.");
        let conflict = update_guardrail(&emulator, &assistant, &guardrail_id, changed).await;
        assert_eq!(
            conflict.status, 409,
            "case {token:?}: one token identifies one request, got HTTP {} with body {}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_type, "ConflictException",
            "case {token:?}: the refusal must carry the modelled error code, got {:?}",
            conflict.error_type
        );
        assert_eq!(
            conflict.string("/message"),
            MESSAGE_IDEMPOTENCY_CONFLICT,
            "case {token:?}: the refusal must carry this endpoint's sentence for the \
             condition, body {}",
            conflict.body
        );
        assert_reads_back(&emulator, token, &assistant, &guardrail_id, &written).await;

        assert_eq!(
            delete_guardrail(&emulator, &assistant, &guardrail_id)
                .await
                .status,
            202,
            "case {token:?}: the cleanup delete must be served"
        );
    }
}
