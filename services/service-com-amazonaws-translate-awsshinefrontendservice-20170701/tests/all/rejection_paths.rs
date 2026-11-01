// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's REFUSAL identities: which error
//! class a refused request names, and the message that class carries.
//!
//! Every test drives the service the way the router does, through the harness
//! `terminology_lifecycle` already builds (`router`, `call`, `error_code`,
//! `message`, `import`), so the wire protocol, the boundary constraint
//! validation, the service's refusal wording and the handlers are all
//! exercised.
//!
//! Each test asserts a RULE over a set of inputs rather than one recorded
//! request: the sizes on both sides of the text limit, several language pairs,
//! each terminology operation's own not-found wording, several rejected enum
//! values, and tag counts around the per-resource tag limit.

use serde_json::{json, Value};

use service_com_amazonaws_translate_awsshinefrontendservice_20170701::ops::types::DISPLAY_LANGUAGE_CODE_VALUES;
use service_com_amazonaws_translate_awsshinefrontendservice_20170701::state::DISPLAY_LANGUAGE_CODE_VALUE_SET;

use crate::terminology_lifecycle::{
    call, error_code, expected_arn, import, message, router, string_at, ACCOUNT, EN_FR_CSV,
};

/// The documented size limit on a translation request's text, in BYTES: "The
/// text string can be a maximum of 10,000 bytes long"
/// (`smithy_query shape=TranslateTextRequest member=Text`).
const MAX_TEXT_BYTES: usize = 10_000;

/// The documented tag limit for one resource: "There can be a maximum of 50
/// tags (both existing and pending) associated with a specific resource"
/// (`smithy_query shape=TagResourceRequest member=Tags`).
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// A paragraph of English, so a request that asks the service to determine its
/// own source language resolves to a concrete code.
const ENGLISH_TEXT: &str = "The quick brown fox jumps over the lazy dog while the gardener \
                            waters the roses in the early morning light.";

fn translate_body(text: &str, source: &str, target: &str) -> Value {
    json!({
        "Text": text,
        "SourceLanguageCode": source,
        "TargetLanguageCode": target,
    })
}

/// The size limit is a BYTE limit, so what decides a refusal is the UTF-8 length
/// of the request's text, not its character count — and the refusal is the
/// operation's own `TextSizeLimitExceededException`, naming the limit and the
/// size that was sent.
///
/// The rows differ in character width as well as in size: a multi-byte text can
/// be well inside the member's character bound and still past the byte limit,
/// which is the case no character-length constraint can decide.
#[tokio::test]
async fn text_past_the_byte_limit_is_refused_as_a_text_size_limit() {
    struct Row {
        case: &'static str,
        text: String,
    }

    let rows = [
        Row {
            case: "a short ASCII text",
            text: "a".repeat(100),
        },
        Row {
            case: "one-byte characters exactly at the limit",
            text: "a".repeat(MAX_TEXT_BYTES),
        },
        Row {
            case: "one-byte characters one past the limit",
            text: "a".repeat(MAX_TEXT_BYTES + 1),
        },
        Row {
            case: "one-byte characters well past the limit",
            text: "a".repeat(MAX_TEXT_BYTES + 137),
        },
        Row {
            case: "two-byte characters exactly at the byte limit",
            text: "\u{e9}".repeat(MAX_TEXT_BYTES / 2),
        },
        Row {
            case: "two-byte characters one character past the byte limit",
            text: "\u{e9}".repeat(MAX_TEXT_BYTES / 2 + 1),
        },
        Row {
            case: "three-byte characters inside the character bound but past the byte limit",
            text: "\u{20ac}".repeat(4_000),
        },
    ];

    let router = router();
    for row in &rows {
        let bytes = row.text.len();
        let refused = bytes > MAX_TEXT_BYTES;
        let (status, body) = call(
            &router,
            "TranslateText",
            &translate_body(&row.text, "en", "es"),
        )
        .await;

        if !refused {
            assert_eq!(
                status, 200,
                "case {:?}: a {bytes}-byte text is inside the limit, got {status} {body}",
                row.case
            );
            assert!(
                string_at(&body, "/TranslatedText").ends_with(&row.text),
                "case {:?}: the translation must carry the text that was sent, got {} characters",
                row.case,
                string_at(&body, "/TranslatedText").chars().count()
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {:?}: a {bytes}-byte text is past the limit, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "TextSizeLimitExceededException",
            "case {:?}: got {body}",
            row.case
        );
        assert_eq!(
            message(&body),
            format!(
                "Input text size exceeds limit. Max length of request text allowed is \
                 {MAX_TEXT_BYTES} bytes while in this request the text size is {bytes} bytes"
            ),
            "case {:?}: the refusal must name the limit and the size sent, got {body}",
            row.case
        );
    }
}

/// A pair the service does not translate is refused with a message naming that
/// pair, built from the codes the service RESOLVED — so a request that asked the
/// service to determine the source language is refused by the code it resolved,
/// not by the word it was asked with.
#[tokio::test]
async fn the_unsupported_pair_refusal_names_the_resolved_pair() {
    struct Row {
        case: &'static str,
        source: &'static str,
        target: &'static str,
        /// The pair the refusal must name, or `None` when the pair is served.
        refused_as: Option<(&'static str, &'static str)>,
    }

    let rows = [
        Row {
            case: "a target language the service does not translate",
            source: "en",
            target: "zz",
            refused_as: Some(("en", "zz")),
        },
        Row {
            case: "a source language the service does not translate",
            source: "zz",
            target: "fr",
            refused_as: Some(("zz", "fr")),
        },
        Row {
            case: "the same language on both sides",
            source: "it",
            target: "it",
            refused_as: Some(("it", "it")),
        },
        Row {
            case: "a regional variant into itself",
            source: "zh-TW",
            target: "zh-TW",
            refused_as: Some(("zh-TW", "zh-TW")),
        },
        Row {
            case: "a detected source language into a target the service does not translate",
            source: "auto",
            target: "zz",
            refused_as: Some(("en", "zz")),
        },
        Row {
            case: "a pair the service does translate",
            source: "de",
            target: "ja",
            refused_as: None,
        },
    ];

    let router = router();
    for row in &rows {
        let (status, body) = call(
            &router,
            "TranslateText",
            &translate_body(ENGLISH_TEXT, row.source, row.target),
        )
        .await;

        let Some((source, target)) = row.refused_as else {
            assert_eq!(
                status, 200,
                "case {:?}: expected the pair to be served, got {status} {body}",
                row.case
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {:?}: expected the pair to be refused, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "UnsupportedLanguagePairException",
            "case {:?}: got {body}",
            row.case
        );
        assert_eq!(
            message(&body),
            format!("Unsupported language pair: {source} to {target}."),
            "case {:?}: the refusal must name the pair it rejected, got {body}",
            row.case
        );
        // The same resolved pair travels in the error shape's own members.
        assert_eq!(
            string_at(&body, "/SourceLanguageCode"),
            source,
            "case {:?}: got {body}",
            row.case
        );
        assert_eq!(
            string_at(&body, "/TargetLanguageCode"),
            target,
            "case {:?}: got {body}",
            row.case
        );
    }
}

/// A terminology no import stored is reported by the resource the OPERATION
/// addresses — the terminology itself for a read or a translation, its metadata
/// record for a delete — together with the account and the name the request
/// asked for.
#[tokio::test]
async fn a_missing_terminology_refusal_names_the_resource_the_operation_addresses() {
    struct Row {
        case: &'static str,
        operation: &'static str,
        name: &'static str,
        resource: &'static str,
    }

    let rows = [
        Row {
            case: "a read of a name no import stored",
            operation: "GetTerminology",
            name: "ghost-terms",
            resource: "Terminology",
        },
        Row {
            case: "a delete of a name no import stored",
            operation: "DeleteTerminology",
            name: "other-ghost-terms",
            resource: "TerminologyMetadata",
        },
        Row {
            case: "a translation naming a terminology no import stored",
            operation: "TranslateText",
            name: "third-ghost-terms",
            resource: "Terminology",
        },
    ];

    let router = router();
    for row in &rows {
        let body = if row.operation == "TranslateText" {
            let mut body = translate_body("hello there", "en", "fr");
            body["TerminologyNames"] = json!([row.name]);
            body
        } else {
            json!({"Name": row.name})
        };
        let (status, answered) = call(&router, row.operation, &body).await;
        assert_eq!(
            status, 404,
            "case {:?}: got {status} {answered}",
            row.case
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {:?}: got {answered}",
            row.case
        );
        assert_eq!(
            message(&answered),
            format!("The {} for {ACCOUNT} {} was not found", row.resource, row.name),
            "case {:?}: got {answered}",
            row.case
        );
    }

    // And for a name that WAS stored and is now gone, the same two wordings:
    // the reader reports the terminology, the delete its metadata record.
    let (status, imported) = import(&router, "short-lived-terms", EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");
    let (status, deleted) =
        call(&router, "DeleteTerminology", &json!({"Name": "short-lived-terms"})).await;
    assert_eq!(status, 200, "DeleteTerminology answered {deleted}");

    let (status, gone) =
        call(&router, "GetTerminology", &json!({"Name": "short-lived-terms"})).await;
    assert_eq!(status, 404, "GetTerminology answered {gone}");
    assert_eq!(
        message(&gone),
        format!("The Terminology for {ACCOUNT} short-lived-terms was not found"),
        "a read after the delete reports the terminology, got {gone}"
    );

    let (status, again) =
        call(&router, "DeleteTerminology", &json!({"Name": "short-lived-terms"})).await;
    assert_eq!(status, 404, "the second delete answered {again}");
    assert_eq!(
        message(&again),
        format!("The TerminologyMetadata for {ACCOUNT} short-lived-terms was not found"),
        "a second delete reports the metadata record, got {again}"
    );
}

/// An out-of-set `DisplayLanguageCode` is refused with the accepted value set
/// stated in the order this service emits it — the SET is the model's, the ORDER
/// is the service's, and only the rejected value varies with the request.
#[tokio::test]
async fn the_enum_refusal_states_the_value_set_in_the_services_order() {
    // The set the service states is exactly the set the model declares
    // (`DISPLAY_LANGUAGE_CODE_VALUES` is generated from it), so the two can
    // only ever differ in order.
    let mut declared: Vec<&str> = DISPLAY_LANGUAGE_CODE_VALUES.to_vec();
    declared.sort_unstable();
    let mut stated: Vec<&str> = DISPLAY_LANGUAGE_CODE_VALUE_SET.to_vec();
    stated.sort_unstable();
    assert_eq!(
        stated, declared,
        "the stated value set must hold exactly the model's values, got {stated:?}"
    );
    assert_ne!(
        DISPLAY_LANGUAGE_CODE_VALUE_SET, DISPLAY_LANGUAGE_CODE_VALUES,
        "the service's order is not the model's declaration order, so the check above \
         must not be comparing one list with itself"
    );

    let expected_set = DISPLAY_LANGUAGE_CODE_VALUE_SET.join(", ");
    let router = router();

    // Rejected values: a code no language uses, another one, and two values
    // that differ from a declared one only in case.
    for value in ["zz", "qq", "EN", "zh-tw"] {
        let (status, body) = call(
            &router,
            "ListLanguages",
            &json!({"DisplayLanguageCode": value}),
        )
        .await;
        assert_eq!(status, 400, "case {value:?}: got {status} {body}");
        assert_eq!(
            message(&body),
            format!(
                "1 validation error detected: Value '{value}' at 'displayLanguageCode' \
                 failed to satisfy constraint: Member must satisfy enum value set: \
                 [{expected_set}]"
            ),
            "case {value:?}: got {body}"
        );
    }

    // Every value the model declares is admitted past that validation. The
    // operation itself is still a scaffold (its own stub issue covers standing
    // it up), so the refusal it answers with must be the scaffold's, never the
    // value-set one.
    for value in DISPLAY_LANGUAGE_CODE_VALUES {
        let (_, body) = call(
            &router,
            "ListLanguages",
            &json!({"DisplayLanguageCode": value}),
        )
        .await;
        assert!(
            !message(&body).contains("enum value set"),
            "case {value:?}: a declared value must not be refused by the value set, got {body}"
        );
    }
}

/// The per-resource tag limit is a rule of the tagging operation, not of the
/// request's shape: a tag list the model admits is refused when the resource
/// would be left holding more tags than the limit, and the refusal applies none
/// of them.
#[tokio::test]
async fn the_tag_limit_is_enforced_over_the_resulting_tag_set() {
    fn tags(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
                .collect(),
        )
    }

    async fn tags_of(router: &smithy_server_runtime::MultiServiceRouter, arn: &str) -> Vec<Value> {
        let (status, body) = call(router, "ListTagsForResource", &json!({"ResourceArn": arn})).await;
        assert_eq!(status, 200, "ListTagsForResource answered {body}");
        body.pointer("/Tags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    let router = router();

    // The rows differ in how many tags one call carries: none, one, exactly the
    // limit, one past it, and well past it — every one inside the `TagList`
    // bound the model declares, so the shape walk admits them all.
    for count in [0usize, 1, MAX_TAGS_PER_RESOURCE, MAX_TAGS_PER_RESOURCE + 1, 60] {
        let name = format!("tagged-{count}-terms");
        let (status, imported) = import(&router, &name, EN_FR_CSV, "CSV", Some("UNI")).await;
        assert_eq!(status, 200, "case {count}: ImportTerminology answered {imported}");
        let arn = expected_arn(&name, EN_FR_CSV);

        let (status, answered) = call(
            &router,
            "TagResource",
            &json!({"ResourceArn": arn, "Tags": tags(count)}),
        )
        .await;

        if count <= MAX_TAGS_PER_RESOURCE {
            assert_eq!(
                status, 200,
                "case {count}: a tag set inside the limit is accepted, got {status} {answered}"
            );
            assert_eq!(
                tags_of(&router, &arn).await.len(),
                count,
                "case {count}: the accepted tags are the ones the resource reports"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {count}: a tag set past the limit is refused, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "TooManyTagsException",
            "case {count}: got {answered}"
        );
        assert_eq!(
            message(&answered),
            "Allowable tag limit for resource exceeded",
            "case {count}: got {answered}"
        );
        assert_eq!(
            string_at(&answered, "/ResourceArn"),
            arn,
            "case {count}: the refusal names the resource it refused, got {answered}"
        );
        assert!(
            tags_of(&router, &arn).await.is_empty(),
            "case {count}: a refused call applies none of its tags"
        );
    }

    // The limit counts the tags the resource already holds together with the
    // ones the call adds, and a key the call restates replaces a value rather
    // than adding a tag.
    let name = "merged-terms";
    let (status, imported) = import(&router, name, EN_FR_CSV, "CSV", Some("UNI")).await;
    assert_eq!(status, 200, "ImportTerminology answered {imported}");
    let arn = expected_arn(name, EN_FR_CSV);

    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": tags(MAX_TAGS_PER_RESOURCE)}),
    )
    .await;
    assert_eq!(status, 200, "TagResource answered {answered}");

    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": [{"Key": "one-too-many", "Value": "v"}]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a resource at the limit takes no further key, got {status} {answered}"
    );
    assert_eq!(error_code(&answered), "TooManyTagsException", "got {answered}");
    assert_eq!(
        tags_of(&router, &arn).await.len(),
        MAX_TAGS_PER_RESOURCE,
        "the refused call leaves the stored tags as they were"
    );

    let (status, answered) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": [{"Key": "k0", "Value": "replaced"}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "re-stating a key the resource holds adds no tag, got {status} {answered}"
    );
    let held = tags_of(&router, &arn).await;
    assert_eq!(
        held.len(),
        MAX_TAGS_PER_RESOURCE,
        "a restated key replaces a value rather than adding a tag, got {held:?}"
    );
    assert!(
        held.contains(&json!({"Key": "k0", "Value": "replaced"})),
        "the restated key carries the new value, got {held:?}"
    );

    // An ARN this account holds no resource for is a not-found refusal from both
    // tagging paths, never an empty tag list.
    let unknown = expected_arn("no-such-terms", b"no-such-file");
    for operation in ["TagResource", "ListTagsForResource"] {
        let (status, answered) = call(
            &router,
            operation,
            &json!({"ResourceArn": unknown, "Tags": tags(1)}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {operation:?}: got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {operation:?}: got {answered}"
        );
    }
}
