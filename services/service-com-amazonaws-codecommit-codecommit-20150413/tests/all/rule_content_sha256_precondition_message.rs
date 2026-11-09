// SPDX-License-Identifier: CC0-1.0

//! What `InvalidRuleContentSha256Exception` SAYS when a conditional update's
//! `existingRuleContentSha256` is not the signature of the content currently stored,
//! over the wire.
//!
//! The rule under test: a content signature a request carries is a PRECONDITION on
//! the content being replaced, so a value that is not the stored content's signature
//! is refused at the status the model gives that class with ONE sentence group —
//! fixed text whose single slot is the signature the CALLER supplied, closed by the
//! read that reports the current signature. The signature the store holds is never
//! disclosed, and nothing about the request other than the supplied value appears in
//! the text: not the resource's name, and not which of the many ways the value failed
//! to be the stored signature.
//!
//! The table therefore spans that class rather than one recorded request: the stored
//! document's shape and size vary, and so does the FORM of the failing signature —
//! the digest of another document, the digest of the replacement, a well-formed digest
//! of nothing stored, a digest one hex character short and one too long, a string of
//! the right length that is not hex, prose, an empty value, and a stale digest in the
//! other hex case. Every row must answer the same template with its own value in the
//! slot, and must leave the stored template exactly as it was.
//!
//! The sibling suite `approval_rule_template_content_update.rs` owns which ERROR this
//! precondition raises and that a matching signature applies; this one owns the TEXT,
//! and re-checks the applying case so the refusals here are not a precondition that
//! refuses everything.
//!
//! The document structure (`Version`, `Statements` of `Type` `Approvers` with
//! `NumberOfApprovalsNeeded`) is the one the CodeCommit User Guide documents for
//! approval rule template content; `ApprovalRuleTemplateContent` carries a modelled
//! `len 1..3000` and `RuleContentSha256` carries no constraint at all
//! (`smithy_query(query="shape",shape="RuleContentSha256")`), which is why a value of
//! any form reaches the comparison.

use serde_json::json;
use sha2::Digest;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{emulator, ok, string_at};

/// The status `InvalidRuleContentSha256Exception` models (`errorKind: client`,
/// `httpError: 400`, defaulted).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The error code this precondition raises, as awsJson carries it in `__type`.
const EXPECTED_CODE: &str = "InvalidRuleContentSha256Exception";

/// The `Version` and statement `Type` an approval rule document carries.
const TEMPLATE_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// An approval rule document with `approvals` approvals needed and the given pool,
/// as compact JSON text.
fn document(approvals: u32, pool: &[&str]) -> String {
    let mut statement = json!({
        "Type": STATEMENT_TYPE,
        "NumberOfApprovalsNeeded": approvals,
    });
    if !pool.is_empty() {
        statement["ApprovalPoolMembers"] = json!(pool);
    }
    json!({ "Version": TEMPLATE_VERSION, "Statements": [statement] }).to_string()
}

/// A document padded with pool members to exactly `total` characters, for the upper
/// end of the modelled `ApprovalRuleTemplateContent` bound of 1..3000.
fn padded_document(total: usize) -> String {
    let prefix = format!(
        "{{\"Version\":\"{TEMPLATE_VERSION}\",\"Statements\":[{{\"Type\":\"{STATEMENT_TYPE}\",\
         \"NumberOfApprovalsNeeded\":1,\"ApprovalPoolMembers\":[\""
    );
    let suffix = "\"]}]}";
    let pad = total - prefix.len() - suffix.len();
    format!("{prefix}{}{suffix}", "p".repeat(pad))
}

/// The digest `ruleContentSha256` reports, computed here independently of the
/// service so a row's signature is built from the rule rather than from the answer
/// under test.
fn content_sha256(content: &str) -> String {
    hex::encode(sha2::Sha256::digest(content.as_bytes()))
}

/// The sentence group CodeCommit answers this class with, written here independently
/// of the service: fixed text whose one slot is the signature the request carried,
/// then the read that reports the current signature.
fn expected_message(supplied: &str) -> String {
    format!(
        "The SHA-256 hash for the current rule content does not match the hash you provided: \
         {supplied}. Use GetApprovalRuleTemplate to get the SHA-256 of the current approval rule."
    )
}

/// The documents a row's template is created with, spanning the shape and size of an
/// approval rule document: the minimum one, a pool of several ARNs, and one at the
/// modelled 3000-character maximum. Each gives the comparison a DIFFERENT stored
/// signature.
fn stored_documents() -> Vec<(&'static str, String)> {
    vec![
        ("one approval, no pool", document(1, &[])),
        (
            "several approvals and a pool",
            document(
                5,
                &[
                    "arn:aws:iam::111122223333:user/one",
                    "arn:aws:sts::111122223333:assumed-role/Release/*",
                ],
            ),
        ),
        ("the 3000-character maximum", padded_document(3000)),
    ]
}

/// The ways a supplied signature can fail to be the stored content's, given what is
/// stored and what would replace it.
fn failing_signatures(stored: &str, replacement: &str) -> Vec<(&'static str, String)> {
    let digest = content_sha256(stored);
    vec![
        (
            "the digest of another document",
            content_sha256(&document(9, &["arn:aws:iam::111122223333:user/other"])),
        ),
        ("the digest of the replacement", content_sha256(replacement)),
        ("a well-formed digest of nothing stored", "0".repeat(64)),
        ("one hex character short", digest[..63].to_string()),
        ("one hex character too long", format!("{digest}a")),
        ("the right length but not hex", "z".repeat(64)),
        ("prose", "not-a-signature".to_string()),
        ("an empty value", String::new()),
        (
            "a stale digest in upper case",
            content_sha256("{}").to_uppercase(),
        ),
    ]
}

#[tokio::test]
async fn a_signature_that_is_not_the_stored_one_is_refused_with_the_caller_s_hash_echoed() {
    let router = emulator();
    let replacement = document(2, &["arn:aws:iam::111122223333:user/reviewer"]);

    for (document_index, (document_case, stored)) in stored_documents().into_iter().enumerate() {
        for (signature_index, (signature_case, supplied)) in
            failing_signatures(&stored, &replacement)
                .into_iter()
                .enumerate()
        {
            let case = format!("{document_case} / {signature_case}");
            let name = format!("guarded-{document_index}-{signature_index}");
            ok(
                &router,
                "CreateApprovalRuleTemplate",
                &json!({
                    "approvalRuleTemplateName": name,
                    "approvalRuleTemplateContent": stored,
                }),
            )
            .await;

            let (code, message) = error_with_message(
                &router,
                "UpdateApprovalRuleTemplateContent",
                &json!({
                    "approvalRuleTemplateName": name,
                    "newRuleContent": replacement,
                    "existingRuleContentSha256": supplied,
                }),
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, EXPECTED_CODE,
                "case {case:?}: expected {EXPECTED_CODE}, got {code:?}"
            );
            assert_eq!(
                message,
                expected_message(&supplied),
                "case {case:?}: expected the class's sentence with {supplied:?} in its slot, \
                 got {message:?}"
            );
            // The stored signature is not the caller's to read off a refusal: the
            // sentence tells it which read reports it instead. The equality above
            // already pins the whole sentence; this states what it is for, on every
            // row whose own value does not embed that digest.
            let stored_digest = content_sha256(&stored);
            if !supplied.contains(&stored_digest) {
                assert!(
                    !message.contains(&stored_digest),
                    "case {case:?}: the refusal must not disclose the stored signature \
                     {stored_digest:?}, got {message:?}"
                );
            }
            assert!(
                !message.contains(&name),
                "case {case:?}: the refusal names no template, got {message:?}"
            );

            // And the refused update wrote nothing.
            let fetched = ok(
                &router,
                "GetApprovalRuleTemplate",
                &json!({ "approvalRuleTemplateName": name }),
            )
            .await;
            let content = string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateContent");
            assert_eq!(
                content, stored,
                "case {case:?}: a refused update must leave the stored content, got {content:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_stored_signature_applies_the_update_in_either_hex_case() {
    let router = emulator();

    for (index, (document_case, stored)) in stored_documents().into_iter().enumerate() {
        for (case_label, upper) in [("as reported", false), ("in upper case", true)] {
            let case = format!("{document_case} / {case_label}");
            let name = format!("applied-{index}-{}", u8::from(upper));
            ok(
                &router,
                "CreateApprovalRuleTemplate",
                &json!({
                    "approvalRuleTemplateName": name,
                    "approvalRuleTemplateContent": stored,
                }),
            )
            .await;

            let digest = content_sha256(&stored);
            let supplied = if upper { digest.to_uppercase() } else { digest };
            let replacement = document(3, &["arn:aws:iam::111122223333:user/applied"]);
            let updated = ok(
                &router,
                "UpdateApprovalRuleTemplateContent",
                &json!({
                    "approvalRuleTemplateName": name,
                    "newRuleContent": replacement,
                    "existingRuleContentSha256": supplied,
                }),
            )
            .await;
            let answered =
                string_at(&updated, "/approvalRuleTemplate/approvalRuleTemplateContent");
            assert_eq!(
                answered, replacement,
                "case {case:?}: the update must store the replacement, got {answered:?}"
            );

            // Read back through the operation the refusal's sentence names: the
            // signature it reports is the one a retry would supply.
            let fetched = ok(
                &router,
                "GetApprovalRuleTemplate",
                &json!({ "approvalRuleTemplateName": name }),
            )
            .await;
            let reported = string_at(&fetched, "/approvalRuleTemplate/ruleContentSha256");
            assert_eq!(
                reported,
                content_sha256(&replacement),
                "case {case:?}: GetApprovalRuleTemplate must report the digest of the stored \
                 content, got {reported:?}"
            );
        }
    }
}
