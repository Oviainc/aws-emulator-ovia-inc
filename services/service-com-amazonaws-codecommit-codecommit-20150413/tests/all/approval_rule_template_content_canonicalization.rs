// SPDX-License-Identifier: CC0-1.0

//! How an approval rule template's JSON document is stored and reported back.
//!
//! The rule under test: a JSON document this service accepts and hands back is
//! stored CANONICALIZED — minified, with every byte of insignificant whitespace
//! (space, tab, CR, LF between tokens) removed and the member order, the string
//! literals and the number literals exactly as submitted — so the create answer,
//! `GetApprovalRuleTemplate` and `ruleContentSha256` all report the same bytes for
//! one document however its submitter formatted it, and canonicalizing an already
//! canonical document changes nothing.
//!
//! The table spans what the rule generalises over: which whitespace characters are
//! used, where they sit (between tokens, inside arrays and objects, around the whole
//! document), whether the submitted member order is the alphabetical one, whitespace
//! and escapes that live INSIDE a string literal (part of the value, so they
//! survive), and the size of the document. The recorded case — a two-space-indented
//! document with destination references — is one row.
//!
//! `UpdateApprovalRuleTemplateContent` is held to the same rule: the content it
//! stores is canonical too, so an update cannot leave a template in a serialization
//! a create would never have produced (that operation was a 501 stub before this
//! change — the issue this suite was written for needed it to report the stored
//! form).

use serde_json::{json, Value};
use sha2::Digest;

// The harness `tests/all/repository_lifecycle.rs` owns and shares with every sibling
// suite: one emulator on one engine, and the awsJson1_1 call helpers.
use crate::repository_lifecycle::{emulator, error, ok, string_at};

/// The SHA-256 hash signature of a document, computed here independently of the
/// service, so `ruleContentSha256` is checked against the rule rather than against
/// itself.
fn content_sha256(content: &str) -> String {
    hex::encode(sha2::Sha256::digest(content.as_bytes()))
}

/// One row: the document as submitted, and the canonical text the service must store
/// and report for it. Every `canonical` here is written out by hand (or built as
/// compact JSON from the same value as the pretty submission), never by running the
/// service's own canonicalization.
#[derive(Debug, Clone)]
struct ContentCase {
    label: &'static str,
    submitted: String,
    canonical: String,
}

/// A document with `count` destination references, submitted pretty-printed and
/// stored as the compact serialization of the same value: the size axis of the
/// table, where the canonical form is dramatically shorter than the submission.
fn wide_case(count: usize) -> ContentCase {
    let references: Vec<String> = (0..count).map(|index| format!("refs/heads/b{index}")).collect();
    let document = json!({
        "Version": "2018-11-08",
        "DestinationReferences": references,
        "Statements": [{
            "Type": "Approvers",
            "NumberOfApprovalsNeeded": 1,
            "ApprovalPoolMembers": ["arn:aws:iam::111122223333:user/reviewer"],
        }],
    });
    ContentCase {
        label: "a wide document submitted pretty-printed",
        submitted: serde_json::to_string_pretty(&document).expect("the document serializes"),
        canonical: document.to_string(),
    }
}

fn content_cases() -> Vec<ContentCase> {
    vec![
        ContentCase {
            label: "a document already minified",
            submitted: r#"{"Version":"2018-11-08","Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":1}]}"#.to_string(),
            canonical: r#"{"Version":"2018-11-08","Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":1}]}"#.to_string(),
        },
        ContentCase {
            // The recorded case: two-space indentation, newlines, and members in an
            // order that is NOT alphabetical.
            label: "two-space indentation and newlines, members out of alphabetical order",
            submitted: "{\n  \"Version\": \"2018-11-08\",\n  \"DestinationReferences\": [\n    \"refs/heads/main\",\n    \"refs/heads/release/*\"\n  ],\n  \"Statements\": [\n    {\n      \"Type\": \"Approvers\",\n      \"NumberOfApprovalsNeeded\": 2,\n      \"ApprovalPoolMembers\": [\n        \"arn:aws:sts::111122223333:assumed-role/CodeCommitReview/*\",\n        \"arn:aws:iam::111122223333:user/Mary_Major\"\n      ]\n    }\n  ]\n}".to_string(),
            canonical: r#"{"Version":"2018-11-08","DestinationReferences":["refs/heads/main","refs/heads/release/*"],"Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":2,"ApprovalPoolMembers":["arn:aws:sts::111122223333:assumed-role/CodeCommitReview/*","arn:aws:iam::111122223333:user/Mary_Major"]}]}"#.to_string(),
        },
        ContentCase {
            label: "tab indentation and CRLF line endings",
            submitted: "{\r\n\t\"Version\": \"2018-11-08\",\r\n\t\"Statements\": [\r\n\t\t{\"Type\": \"Approvers\", \"NumberOfApprovalsNeeded\": 3}\r\n\t]\r\n}".to_string(),
            canonical: r#"{"Version":"2018-11-08","Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":3}]}"#.to_string(),
        },
        ContentCase {
            label: "a space on both sides of every structural character",
            submitted: r#" { "Version" : "2018-11-08" , "Statements" : [ { "Type" : "Approvers" , "NumberOfApprovalsNeeded" : 1 , "ApprovalPoolMembers" : [ "arn:aws:iam::111122223333:user/one" , "arn:aws:iam::111122223333:user/two" ] } ] } "#.to_string(),
            canonical: r#"{"Version":"2018-11-08","Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":1,"ApprovalPoolMembers":["arn:aws:iam::111122223333:user/one","arn:aws:iam::111122223333:user/two"]}]}"#.to_string(),
        },
        ContentCase {
            label: "whitespace only around the whole document",
            submitted: "\n\n\t{\"Version\":\"2018-11-08\",\"Statements\":[{\"Type\":\"Approvers\",\"NumberOfApprovalsNeeded\":9}]}\r\n   ".to_string(),
            canonical: r#"{"Version":"2018-11-08","Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":9}]}"#.to_string(),
        },
        ContentCase {
            // Whitespace inside a string literal is part of the VALUE, so it stays:
            // only whitespace between tokens is insignificant.
            label: "spaces inside string literals are part of the value",
            submitted: "{\n  \"Version\": \"2018-11-08\",\n  \"DestinationReferences\": [\"refs/heads/release notes\"],\n  \"Statements\": [{\"Type\": \"Approvers\", \"NumberOfApprovalsNeeded\": 1, \"ApprovalPoolMembers\": [\"arn:aws:iam::111122223333:user/Mary Major\"]}]\n}".to_string(),
            canonical: r#"{"Version":"2018-11-08","DestinationReferences":["refs/heads/release notes"],"Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":1,"ApprovalPoolMembers":["arn:aws:iam::111122223333:user/Mary Major"]}]}"#.to_string(),
        },
        ContentCase {
            // Escapes inside a string literal survive exactly as written — an
            // escaped quote does not end the literal, and a `\n`, `\t` or `\u0020`
            // escape is two characters of the value, not whitespace to strip.
            label: "escapes and structural characters inside a string literal",
            submitted: "{\n  \"Version\": \"2018-11-08\",\n  \"Statements\": [{\n    \"Type\": \"Approvers\",\n    \"NumberOfApprovalsNeeded\": 1,\n    \"ApprovalPoolMembers\": [\"quoted \\\" brace { colon : backslash \\\\ tab \\t newline \\n space \\u0020 end\"]\n  }]\n}".to_string(),
            canonical: "{\"Version\":\"2018-11-08\",\"Statements\":[{\"Type\":\"Approvers\",\"NumberOfApprovalsNeeded\":1,\"ApprovalPoolMembers\":[\"quoted \\\" brace { colon : backslash \\\\ tab \\t newline \\n space \\u0020 end\"]}]}".to_string(),
        },
        wide_case(30),
    ]
}

#[tokio::test]
async fn template_content_is_stored_and_reported_in_canonical_minified_form() {
    let router = emulator();

    for (index, case) in content_cases().into_iter().enumerate() {
        let label = case.label;
        assert!(
            case.submitted.len() <= 3000,
            "case {label:?}: the submitted document must fit the modelled \
             ApprovalRuleTemplateContent maximum of 3000, got {} characters",
            case.submitted.len()
        );

        let name = format!("canonical-{index}");
        let created = ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": name,
                "approvalRuleTemplateContent": case.submitted,
            }),
        )
        .await;
        let stored = string_at(&created, "/approvalRuleTemplate/approvalRuleTemplateContent");
        assert_eq!(
            stored, case.canonical,
            "case {label:?}: the created content must be the canonical minified document, \
             got {stored:?}"
        );
        assert_eq!(
            string_at(&created, "/approvalRuleTemplate/ruleContentSha256"),
            content_sha256(&case.canonical),
            "case {label:?}: ruleContentSha256 must be the digest of the canonical content"
        );
        assert!(
            stored.len() <= case.submitted.len(),
            "case {label:?}: the canonical document is never longer than the submission, \
             got {} against {} characters",
            stored.len(),
            case.submitted.len()
        );

        // Values and structure are untouched: the canonical text parses to exactly
        // the submitted document.
        let submitted_value: Value =
            serde_json::from_str(&case.submitted).expect("the case document is JSON");
        let stored_value: Value =
            serde_json::from_str(&stored).unwrap_or_else(|error| {
                panic!("case {label:?}: the stored content must still be JSON: {error}")
            });
        assert_eq!(
            stored_value, submitted_value,
            "case {label:?}: canonicalization changes no value, got {stored:?}"
        );

        // Member order is the SUBMITTED order, not the order a parse-and-dump would
        // impose: where the submission is not in alphabetical member order, the
        // stored text differs from that re-serialization.
        let re_serialized = submitted_value.to_string();
        if re_serialized != case.canonical {
            assert_ne!(
                stored, re_serialized,
                "case {label:?}: the submitted member order must be preserved"
            );
        }

        // The same bytes come back from the reader: the canonicalization is in what
        // was PERSISTED, not in one response.
        let fetched = ok(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": name }),
        )
        .await;
        assert_eq!(
            fetched.get("approvalRuleTemplate"),
            created.get("approvalRuleTemplate"),
            "case {label:?}: the get must report what the create stored"
        );

        // Canonicalization is idempotent: submitting the canonical text stores and
        // reports that same text, with the same signature.
        let again = ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": format!("{name}-recanonicalized"),
                "approvalRuleTemplateContent": case.canonical,
            }),
        )
        .await;
        assert_eq!(
            string_at(&again, "/approvalRuleTemplate/approvalRuleTemplateContent"),
            case.canonical,
            "case {label:?}: canonicalizing a canonical document changes nothing"
        );
        assert_eq!(
            string_at(&again, "/approvalRuleTemplate/ruleContentSha256"),
            content_sha256(&case.canonical),
            "case {label:?}: one document has one signature however it was submitted"
        );
    }
}

/// The document every update case starts from: already minified, and different from
/// every row's canonical form, so an update is always a real change.
const BASE_DOCUMENT: &str = r#"{"Version":"2018-11-08","Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":7,"ApprovalPoolMembers":["arn:aws:iam::111122223333:user/base"]}]}"#;

#[tokio::test]
async fn updated_template_content_is_stored_in_canonical_minified_form() {
    let router = emulator();

    for (index, case) in content_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("updated-{index}");
        let created = ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": name,
                "approvalRuleTemplateContent": BASE_DOCUMENT,
            }),
        )
        .await;
        assert_eq!(
            string_at(&created, "/approvalRuleTemplate/approvalRuleTemplateContent"),
            BASE_DOCUMENT,
            "case {label:?}: the base document is stored as submitted"
        );

        // A signature that is not the one of the stored content is refused, and
        // nothing is written.
        let stale = content_sha256("{}");
        for (signature_case, signature) in [
            ("a signature of other content", stale.as_str()),
            ("a signature that is not a digest at all", "not-a-sha256"),
        ] {
            let code = error(
                &router,
                "UpdateApprovalRuleTemplateContent",
                &json!({
                    "approvalRuleTemplateName": name,
                    "newRuleContent": case.submitted,
                    "existingRuleContentSha256": signature,
                }),
                400,
            )
            .await;
            assert_eq!(
                code, "InvalidRuleContentSha256Exception",
                "case {label:?}/{signature_case:?}: expected \
                 InvalidRuleContentSha256Exception, got {code:?}"
            );
            let unchanged = ok(
                &router,
                "GetApprovalRuleTemplate",
                &json!({ "approvalRuleTemplateName": name }),
            )
            .await;
            assert_eq!(
                string_at(&unchanged, "/approvalRuleTemplate/approvalRuleTemplateContent"),
                BASE_DOCUMENT,
                "case {label:?}/{signature_case:?}: a refused update writes nothing"
            );
        }

        // The update, naming the signature of what it replaces: the new content is
        // stored canonicalized, exactly as a create would have stored it.
        let updated = ok(
            &router,
            "UpdateApprovalRuleTemplateContent",
            &json!({
                "approvalRuleTemplateName": name,
                "newRuleContent": case.submitted,
                "existingRuleContentSha256": content_sha256(BASE_DOCUMENT),
            }),
        )
        .await;
        let stored = string_at(&updated, "/approvalRuleTemplate/approvalRuleTemplateContent");
        assert_eq!(
            stored, case.canonical,
            "case {label:?}: the updated content must be the canonical minified document, \
             got {stored:?}"
        );
        assert_eq!(
            string_at(&updated, "/approvalRuleTemplate/ruleContentSha256"),
            content_sha256(&case.canonical),
            "case {label:?}: ruleContentSha256 must be the digest of the updated content"
        );
        assert_eq!(
            string_at(&updated, "/approvalRuleTemplate/approvalRuleTemplateId"),
            string_at(&created, "/approvalRuleTemplate/approvalRuleTemplateId"),
            "case {label:?}: an update replaces content, not the template"
        );

        // And that is what the store holds: the reader reports the same projection.
        let fetched = ok(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": name }),
        )
        .await;
        assert_eq!(
            fetched.get("approvalRuleTemplate"),
            updated.get("approvalRuleTemplate"),
            "case {label:?}: the get must report what the update stored"
        );

        // The update is also accepted with no signature at all (the member is
        // optional), and canonicalizing the canonical text changes nothing.
        let again = ok(
            &router,
            "UpdateApprovalRuleTemplateContent",
            &json!({
                "approvalRuleTemplateName": name,
                "newRuleContent": case.canonical,
            }),
        )
        .await;
        assert_eq!(
            string_at(&again, "/approvalRuleTemplate/approvalRuleTemplateContent"),
            case.canonical,
            "case {label:?}: re-submitting the canonical document changes nothing"
        );
    }

    // An update to a template nobody created is refused, whatever the content.
    let code = error(
        &router,
        "UpdateApprovalRuleTemplateContent",
        &json!({
            "approvalRuleTemplateName": "no-such-template",
            "newRuleContent": BASE_DOCUMENT,
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "ApprovalRuleTemplateDoesNotExistException",
        "case \"an absent template\": expected ApprovalRuleTemplateDoesNotExistException, \
         got {code:?}"
    );

    // Content that is not an approval rule document is refused by the update exactly
    // as the create refuses it, and the stored content survives.
    let name = "updated-0";
    for (bad_case, content) in [
        ("prose that is not JSON", "not a document at all"),
        ("a JSON array", "[]"),
        (
            "a Version the schema does not define",
            r#"{"Version":"2525-01-01","Statements":[{"Type":"Approvers","NumberOfApprovalsNeeded":1}]}"#,
        ),
    ] {
        let code = error(
            &router,
            "UpdateApprovalRuleTemplateContent",
            &json!({
                "approvalRuleTemplateName": name,
                "newRuleContent": content,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidApprovalRuleTemplateContentException",
            "case {bad_case:?}: expected InvalidApprovalRuleTemplateContentException, \
             got {code:?}"
        );
    }
    let code = error(
        &router,
        "UpdateApprovalRuleTemplateContent",
        &json!({
            "approvalRuleTemplateName": name,
            "newRuleContent": "   \n\t",
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "ApprovalRuleTemplateContentRequiredException",
        "case \"content of nothing but whitespace\": expected \
         ApprovalRuleTemplateContentRequiredException, got {code:?}"
    );
}
