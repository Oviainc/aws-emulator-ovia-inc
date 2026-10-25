// SPDX-License-Identifier: CC0-1.0

//! How ECR reports an ECR lifecycle-policy document back, and the evaluation instant that
//! travels with it.
//!
//! Every case goes through the shared `wire` harness, so it exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: `lifecyclePolicyText` is an opaque document
//! (`ecr#LifecyclePolicyText`, a string of 100..30720 characters, per
//! `smithy_query(query="operation", operation="PutLifecyclePolicy")`), and every operation
//! that reports one reports it MINIFIED — the document it was given with the whitespace
//! between its JSON tokens removed, and nothing else about it changed: the members keep
//! the order they arrived in, every escape and every number keeps its spelling, and the
//! whitespace inside a string is part of the string. The same bytes come back from the put
//! that stored it, the read, the preview start, the preview read and the delete. Two of
//! those operations also model `lastEvaluatedAt` (`GetLifecyclePolicy`,
//! `DeleteLifecyclePolicy`): they report the instant the policy was last evaluated, which
//! a read never moves.
//!
//! The table is therefore a ROUND-TRIP invariant stated as `(sent bytes, reported bytes)`,
//! with rows differing along what the rule generalises over: where the whitespace is
//! (after a separator, as indentation, around the document, inside a string), what the
//! document says (member order, rule count, count type), how its text is spelled (ASCII,
//! non-ASCII, `\u` escapes) and how long it is (the template's shortest document, and the
//! published maximum).

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, ok, refused, REGISTRY_ID};

/// The published bounds of `lifecyclePolicyText` (`@length(min: 100, max: 30720)` on
/// `ecr#LifecyclePolicyText`).
const MIN_TEXT_LEN: usize = 100;
/// See [`MIN_TEXT_LEN`].
const MAX_TEXT_LEN: usize = 30_720;

/// The one-rule policy every formatting row carries, minified: the spelling ECR reports
/// for each of them.
const ONE_RULE_MINIFIED: &str = "{\"rules\":[{\"rulePriority\":1,\"description\":\"keep the \
     newest thirty\",\"selection\":{\"tagStatus\":\"any\",\"countType\":\
     \"imageCountMoreThan\",\"countNumber\":30},\"action\":{\"type\":\"expire\"}}]}";

/// A repository to hang a policy on, created through `CreateRepository` so every case
/// only ever observes persisted state.
async fn repository(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        name,
        "CreateRepository",
        &json!({ "repositoryName": name }),
    )
    .await;
}

/// Seconds since the epoch, for bracketing an instant the service generated.
fn now_epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A minified one-rule policy document whose `description` is a run of `x`, exactly `len`
/// characters long, so a case can sit on a published end of the length bound.
fn document_of_length(len: usize) -> String {
    let head = "{\"rules\":[{\"rulePriority\":1,\"description\":\"";
    let tail = "\",\"selection\":{\"tagStatus\":\"any\",\"countType\":\"imageCountMoreThan\",\
                \"countNumber\":30},\"action\":{\"type\":\"expire\"}}]}";
    assert!(
        len >= head.len() + tail.len(),
        "a document of {len} characters cannot carry one rule"
    );
    let text = format!("{head}{}{tail}", "x".repeat(len - head.len() - tail.len()));
    assert_eq!(
        text.chars().count(),
        len,
        "the padded document must be exactly {len} characters long"
    );
    text
}

/// A well-formed JSON object exactly `len` characters long, padded with the whitespace a
/// JSON parser ignores. It carries an EMPTY `rules` array, so the lifecycle policy
/// template refuses it: a row built from this can only be accepted by something that
/// never read it.
fn short_json(len: usize) -> String {
    let document = "{\"rules\":[]}";
    assert!(
        len >= document.len(),
        "a document of {len} characters cannot carry an empty rules array"
    );
    let text = format!("{document}{}", " ".repeat(len - document.len()));
    assert_eq!(
        text.chars().count(),
        len,
        "the padded document must be exactly {len} characters long"
    );
    text
}

/// One row: the case, the bytes the client sends, and the bytes every operation that
/// reports that policy must answer with.
struct Document {
    case: &'static str,
    sent: String,
    reported: String,
}

/// The table: one row per way a document can differ in bytes or in what it says.
fn documents() -> Vec<Document> {
    let row = |case: &'static str, sent: &str, reported: &str| Document {
        case,
        sent: sent.to_string(),
        reported: reported.to_string(),
    };
    vec![
        // Already minified: nothing to remove, so the bytes come back as they went in.
        row(
            "a document already carrying no whitespace",
            ONE_RULE_MINIFIED,
            ONE_RULE_MINIFIED,
        ),
        // A space after every separator — what a client whose JSON writer separates with
        // `", "`/`": "` sends.
        row(
            "a space after every separator",
            "{\"rules\": [{\"rulePriority\": 1, \"description\": \"keep the newest thirty\", \
             \"selection\": {\"tagStatus\": \"any\", \"countType\": \"imageCountMoreThan\", \
             \"countNumber\": 30}, \"action\": {\"type\": \"expire\"}}]}",
            ONE_RULE_MINIFIED,
        ),
        row(
            "indented over several lines",
            "{\n  \"rules\": [\n    {\n      \"rulePriority\": 1,\n      \"description\": \
             \"keep the newest thirty\",\n      \"selection\": {\n        \"tagStatus\": \
             \"any\",\n        \"countType\": \"imageCountMoreThan\",\n        \
             \"countNumber\": 30\n      },\n      \"action\": {\n        \"type\": \
             \"expire\"\n      }\n    }\n  ]\n}",
            ONE_RULE_MINIFIED,
        ),
        row(
            "wrapped in whitespace a JSON parser ignores",
            "\n\t  {\"rules\":[{\"rulePriority\":1,\"description\":\"keep the newest \
             thirty\",\"selection\":{\"tagStatus\":\"any\",\"countType\":\
             \"imageCountMoreThan\",\"countNumber\":30},\"action\":{\"type\":\"expire\"}}]} \
             \n",
            ONE_RULE_MINIFIED,
        ),
        // The members keep the order they arrived in: a document that lists them
        // backwards comes back backwards, only without the spaces.
        row(
            "the rule's members in the reverse of the documented order",
            "{\"rules\": [{\"action\": {\"type\": \"expire\"}, \"selection\": \
             {\"countNumber\": 30, \"countType\": \"imageCountMoreThan\", \"tagStatus\": \
             \"any\"}, \"description\": \"keep the newest thirty\", \"rulePriority\": 1}]}",
            "{\"rules\":[{\"action\":{\"type\":\"expire\"},\"selection\":{\"countNumber\":30,\
             \"countType\":\"imageCountMoreThan\",\"tagStatus\":\"any\"},\"description\":\
             \"keep the newest thirty\",\"rulePriority\":1}]}",
        ),
        // Whitespace INSIDE a string is part of the string, separators and all.
        row(
            "whitespace and separators inside a string",
            "{\"rules\": [{\"rulePriority\": 1, \"description\": \"keep:  the newest, \
             thirty\\t\", \"selection\": {\"tagStatus\": \"any\", \"countType\": \
             \"imageCountMoreThan\", \"countNumber\": 30}, \"action\": {\"type\": \
             \"expire\"}}]}",
            "{\"rules\":[{\"rulePriority\":1,\"description\":\"keep:  the newest, thirty\\t\",\
             \"selection\":{\"tagStatus\":\"any\",\"countType\":\"imageCountMoreThan\",\
             \"countNumber\":30},\"action\":{\"type\":\"expire\"}}]}",
        ),
        // Text outside ASCII is reported as it arrived, not re-escaped.
        row(
            "a description outside ASCII",
            "{\"rules\": [{\"rulePriority\": 1, \"description\": \"garder les trente \
             dernières images — 保持三十\", \"selection\": {\"tagStatus\": \"any\", \
             \"countType\": \"imageCountMoreThan\", \"countNumber\": 30}, \"action\": \
             {\"type\": \"expire\"}}]}",
            "{\"rules\":[{\"rulePriority\":1,\"description\":\"garder les trente dernières \
             images — 保持三十\",\"selection\":{\"tagStatus\":\"any\",\"countType\":\
             \"imageCountMoreThan\",\"countNumber\":30},\"action\":{\"type\":\"expire\"}}]}",
        ),
        // An escape keeps its spelling: it is a token of the document, not whitespace
        // between two.
        row(
            "a description written with \\u escapes",
            "{\"rules\": [{\"rulePriority\": 1, \"description\": \
             \"keep\\u0020the\\u0020newest\\u0020thirty\", \"selection\": {\"tagStatus\": \
             \"any\", \"countType\": \"imageCountMoreThan\", \"countNumber\": 30}, \
             \"action\": {\"type\": \"expire\"}}]}",
            "{\"rules\":[{\"rulePriority\":1,\"description\":\
             \"keep\\u0020the\\u0020newest\\u0020thirty\",\"selection\":{\"tagStatus\":\
             \"any\",\"countType\":\"imageCountMoreThan\",\"countNumber\":30},\"action\":\
             {\"type\":\"expire\"}}]}",
        ),
        // What the document SAYS is another axis: two rules instead of one, and a count of
        // days instead of a count of images.
        row(
            "two rules, the catch-all evaluated last",
            "{\"rules\": [{\"rulePriority\": 1, \"selection\": {\"tagStatus\": \"tagged\", \
             \"tagPrefixList\": [\"prod\", \"release\"], \"countType\": \
             \"imageCountMoreThan\", \"countNumber\": 50}, \"action\": {\"type\": \
             \"expire\"}}, {\"rulePriority\": 2, \"selection\": {\"tagStatus\": \"any\", \
             \"countType\": \"imageCountMoreThan\", \"countNumber\": 100}, \"action\": \
             {\"type\": \"expire\"}}]}",
            "{\"rules\":[{\"rulePriority\":1,\"selection\":{\"tagStatus\":\"tagged\",\
             \"tagPrefixList\":[\"prod\",\"release\"],\"countType\":\"imageCountMoreThan\",\
             \"countNumber\":50},\"action\":{\"type\":\"expire\"}},{\"rulePriority\":2,\
             \"selection\":{\"tagStatus\":\"any\",\"countType\":\"imageCountMoreThan\",\
             \"countNumber\":100},\"action\":{\"type\":\"expire\"}}]}",
        ),
        row(
            "a rule counting days since the image was pushed",
            "{\"rules\": [{\"rulePriority\": 1, \"description\": \"expire untagged images\", \
             \"selection\": {\"tagStatus\": \"untagged\", \"countType\": \
             \"sinceImagePushed\", \"countUnit\": \"days\", \"countNumber\": 14}, \
             \"action\": {\"type\": \"expire\"}}]}",
            "{\"rules\":[{\"rulePriority\":1,\"description\":\"expire untagged images\",\
             \"selection\":{\"tagStatus\":\"untagged\",\"countType\":\"sinceImagePushed\",\
             \"countUnit\":\"days\",\"countNumber\":14},\"action\":{\"type\":\"expire\"}}]}",
        ),
        // The length axis. The published MINIMUM (100) is shorter than any document the
        // template admits, so the shortest row is the template's own floor: one rule with
        // nothing optional on it.
        Document {
            case: "the shortest document the template admits",
            sent: "{\"rules\":[{\"rulePriority\":1,\"selection\":{\"tagStatus\":\"any\",\
                   \"countType\":\"imageCountMoreThan\",\"countNumber\":30},\"action\":\
                   {\"type\":\"expire\"}}]}"
                .to_string(),
            reported: "{\"rules\":[{\"rulePriority\":1,\"selection\":{\"tagStatus\":\"any\",\
                       \"countType\":\"imageCountMoreThan\",\"countNumber\":30},\"action\":\
                       {\"type\":\"expire\"}}]}"
                .to_string(),
        },
        Document {
            case: "a document at the published maximum length",
            sent: document_of_length(MAX_TEXT_LEN),
            reported: document_of_length(MAX_TEXT_LEN),
        },
    ]
}

#[tokio::test]
async fn every_operation_that_reports_a_lifecycle_policy_reports_the_minified_document() {
    let router = emulator();

    for (index, document) in documents().into_iter().enumerate() {
        let Document {
            case,
            sent,
            reported,
        } = document;
        // One repository per row: a repository carries one lifecycle policy preview at a
        // time and a start while one is still running is refused, so each row starts its
        // own.
        let name = format!("document-identity-{index}");
        repository(&router, &name).await;

        // The row must be honest: the two spellings are the same JSON document, and the
        // one that is sent satisfies the modelled length bound, so the case is about the
        // bytes and nothing else.
        let sent_value: Value = serde_json::from_str(&sent)
            .unwrap_or_else(|error| panic!("case {case:?}: the row is not JSON: {error}"));
        let reported_value: Value = serde_json::from_str(&reported).unwrap_or_else(|error| {
            panic!("case {case:?}: the row's reported spelling is not JSON: {error}")
        });
        assert_eq!(
            sent_value, reported_value,
            "case {case:?}: the two spellings of the row must be the same document"
        );
        let length = sent.chars().count();
        assert!(
            (MIN_TEXT_LEN..=MAX_TEXT_LEN).contains(&length),
            "case {case:?}: the row must lie inside the published length bound, got \
             {length} characters"
        );

        let put = ok(
            &router,
            case,
            "PutLifecyclePolicy",
            &json!({ "repositoryName": name, "lifecyclePolicyText": sent }),
        )
        .await;
        assert_eq!(
            put["lifecyclePolicyText"].as_str(),
            Some(reported.as_str()),
            "case {case:?}: the put must report the minified document, got {:?}",
            put["lifecyclePolicyText"]
        );
        // `PutLifecyclePolicyResponse` models no `lastEvaluatedAt`, so the put reports
        // none however the policy was stored.
        assert!(
            put["lastEvaluatedAt"].is_null(),
            "case {case:?}: the put's output models no lastEvaluatedAt, got {put:?}"
        );

        let got = ok(
            &router,
            case,
            "GetLifecyclePolicy",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            got["lifecyclePolicyText"].as_str(),
            Some(reported.as_str()),
            "case {case:?}: the read must report the stored document, got {:?}",
            got["lifecyclePolicyText"]
        );

        // A start that names no document previews the stored policy, and reports it.
        let started = ok(
            &router,
            case,
            "StartLifecyclePolicyPreview",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            started["lifecyclePolicyText"].as_str(),
            Some(reported.as_str()),
            "case {case:?}: the preview start must report the stored document, got {:?}",
            started["lifecyclePolicyText"]
        );

        let preview = ok(
            &router,
            case,
            "GetLifecyclePolicyPreview",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            preview["lifecyclePolicyText"].as_str(),
            Some(reported.as_str()),
            "case {case:?}: the preview read must report the previewed document, got {:?}",
            preview["lifecyclePolicyText"]
        );

        let deleted = ok(
            &router,
            case,
            "DeleteLifecyclePolicy",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            deleted["lifecyclePolicyText"].as_str(),
            Some(reported.as_str()),
            "case {case:?}: the delete must report the document it removed, got {:?}",
            deleted["lifecyclePolicyText"]
        );
    }
}

#[tokio::test]
async fn a_document_a_preview_start_names_itself_is_reported_minified_too() {
    let router = emulator();

    // A document the start call names is not stored anywhere, so how it is reported is
    // the whole observable behaviour — and it is reported exactly as a stored one is.
    for (index, document) in documents().into_iter().enumerate() {
        let name = format!("named-document-{index}");
        repository(&router, &name).await;

        let started = ok(
            &router,
            document.case,
            "StartLifecyclePolicyPreview",
            &json!({ "repositoryName": name, "lifecyclePolicyText": document.sent }),
        )
        .await;
        assert_eq!(
            started["lifecyclePolicyText"].as_str(),
            Some(document.reported.as_str()),
            "case {:?}: the preview start must report the minified document it was given, \
             got {:?}",
            document.case,
            started["lifecyclePolicyText"]
        );

        let preview = ok(
            &router,
            document.case,
            "GetLifecyclePolicyPreview",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            preview["lifecyclePolicyText"].as_str(),
            Some(document.reported.as_str()),
            "case {:?}: the preview read must report the previewed document, got {:?}",
            document.case,
            preview["lifecyclePolicyText"]
        );
    }
}

#[tokio::test]
async fn the_evaluation_instant_is_set_when_the_policy_is_set_and_unchanged_by_a_read() {
    let router = emulator();
    let name = "document-identity-evaluation";
    repository(&router, name).await;

    let table = documents();
    let first = &table[0];
    let second = &table[1];

    let before = now_epoch_secs();
    ok(
        &router,
        first.case,
        "PutLifecyclePolicy",
        &json!({ "repositoryName": name, "lifecyclePolicyText": first.sent }),
    )
    .await;
    let after = now_epoch_secs();

    let got = ok(
        &router,
        first.case,
        "GetLifecyclePolicy",
        &json!({ "repositoryName": name }),
    )
    .await;
    let evaluated = got["lastEvaluatedAt"].as_f64().unwrap_or_else(|| {
        panic!(
            "case {:?}: the read must report a lastEvaluatedAt timestamp, got {:?}",
            first.case, got["lastEvaluatedAt"]
        )
    });
    assert!(
        evaluated >= before - 1.0 && evaluated <= after + 1.0,
        "case {:?}: lastEvaluatedAt must be the instant the policy was set, between \
         {before} and {after}, got {evaluated}",
        first.case
    );

    // A read is a read: the instant it reports does not move because somebody read it.
    for attempt in ["second read", "third read"] {
        let again = ok(
            &router,
            attempt,
            "GetLifecyclePolicy",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            again["lastEvaluatedAt"].as_f64(),
            Some(evaluated),
            "case {attempt:?}: a read must not move lastEvaluatedAt, got {:?}",
            again["lastEvaluatedAt"]
        );
    }

    // Setting a policy again evaluates the new document: the instant is at or after the
    // first one, and belongs to the replacement.
    ok(
        &router,
        second.case,
        "PutLifecyclePolicy",
        &json!({ "repositoryName": name, "lifecyclePolicyText": second.sent }),
    )
    .await;
    let replaced = ok(
        &router,
        second.case,
        "GetLifecyclePolicy",
        &json!({ "repositoryName": name }),
    )
    .await;
    let replaced_evaluated = replaced["lastEvaluatedAt"].as_f64().unwrap_or_else(|| {
        panic!(
            "case {:?}: the read must report a lastEvaluatedAt timestamp, got {:?}",
            second.case, replaced["lastEvaluatedAt"]
        )
    });
    assert!(
        replaced_evaluated >= evaluated,
        "case {:?}: a replacement is evaluated no earlier than what it replaced, got \
         {replaced_evaluated} after {evaluated}",
        second.case
    );
    assert_eq!(
        replaced["lifecyclePolicyText"].as_str(),
        Some(second.reported.as_str()),
        "case {:?}: the read must report the replacement, got {:?}",
        second.case,
        replaced["lifecyclePolicyText"]
    );

    // The delete is the last place the evaluation instant of the policy it removes can be
    // read, and it reports the one that was stored.
    let deleted = ok(
        &router,
        second.case,
        "DeleteLifecyclePolicy",
        &json!({ "repositoryName": name }),
    )
    .await;
    assert_eq!(
        deleted["lastEvaluatedAt"].as_f64(),
        Some(replaced_evaluated),
        "case {:?}: the delete must report the evaluation instant of the policy it \
         removed, got {:?}",
        second.case,
        deleted["lastEvaluatedAt"]
    );
}

#[tokio::test]
async fn a_document_past_a_published_length_bound_is_refused_and_stores_nothing() {
    let router = emulator();
    let name = "document-identity-bounds";
    repository(&router, name).await;

    let stored = document_of_length(MAX_TEXT_LEN);
    ok(
        &router,
        "a document at the published maximum length",
        "PutLifecyclePolicy",
        &json!({ "repositoryName": name, "lifecyclePolicyText": stored }),
    )
    .await;

    // One character past each published end of the bound. No document the template admits
    // is shorter than the minimum, so the under-minimum row is a well-formed JSON object
    // the template would REFUSE (`rules` carries no rule): were the handler reading it the
    // message would be about the document's schema, so the message asserted below is what
    // pins the member's own length bound as the thing that refuses it, at the boundary.
    for (case, text, expected_message) in [
        (
            "one character below the published minimum",
            short_json(MIN_TEXT_LEN - 1),
            format!(
                "Invalid parameter at 'lifecyclePolicyText' failed to satisfy constraint: \
                 'Member must have length greater than or equal to {MIN_TEXT_LEN}'"
            ),
        ),
        (
            "one character above the published maximum",
            document_of_length(MAX_TEXT_LEN + 1),
            format!(
                "Invalid parameter at 'lifecyclePolicyText' failed to satisfy constraint: \
                 'Member must have length less than or equal to {MAX_TEXT_LEN}'"
            ),
        ),
    ] {
        let answer = call(
            &router,
            "PutLifecyclePolicy",
            &json!({ "repositoryName": name, "lifecyclePolicyText": text }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a document outside the modelled length bound is refused, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {case:?}: a parameter outside its bound is an invalid parameter, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            expected_message,
            "case {case:?}: the refusal must name the member and the bound it broke, got \
             {:?}",
            answer.body
        );
        let got = ok(
            &router,
            case,
            "GetLifecyclePolicy",
            &json!({ "repositoryName": name }),
        )
        .await;
        assert_eq!(
            got["lifecyclePolicyText"].as_str(),
            Some(stored.as_str()),
            "case {case:?}: a refused put must leave the stored document untouched, got {:?}",
            got["lifecyclePolicyText"]
        );
    }
}

#[tokio::test]
async fn a_missing_policy_or_preview_is_refused_with_the_wording_that_names_what_is_absent() {
    let router = emulator();
    let name = "document-identity-absent";
    repository(&router, name).await;

    // Both messages are built from the request: each names the thing that does not exist,
    // the repository that was asked about and the registry it was asked in.
    let policy_wording = format!(
        "Lifecycle policy does not exist for the repository with name '{name}' in the \
         registry with id '{REGISTRY_ID}'"
    );
    let preview_wording = format!(
        "Lifecycle policy preview does not exist for the repository with name '{name}' in \
         the registry with id '{REGISTRY_ID}'"
    );

    for operation in ["GetLifecyclePolicy", "DeleteLifecyclePolicy"] {
        let body = refused(
            &router,
            "no policy set",
            operation,
            &json!({ "repositoryName": name }),
            "LifecyclePolicyNotFoundException",
        )
        .await;
        assert_eq!(
            body["message"].as_str(),
            Some(policy_wording.as_str()),
            "case {operation:?}: the refusal must name the policy, the repository and the \
             registry, got {:?}",
            body["message"]
        );
    }

    let body = refused(
        &router,
        "no preview started",
        "GetLifecyclePolicyPreview",
        &json!({ "repositoryName": name }),
        "LifecyclePolicyPreviewNotFoundException",
    )
    .await;
    assert_eq!(
        body["message"].as_str(),
        Some(preview_wording.as_str()),
        "case {:?}: the refusal must name the preview, the repository and the registry, \
         got {:?}",
        "no preview started",
        body["message"]
    );
}
