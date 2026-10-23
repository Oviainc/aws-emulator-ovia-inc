// SPDX-License-Identifier: CC0-1.0

//! `UpdateApprovalRuleTemplateContent` over the wire, driven as awsJson1_1 requests
//! through the router that serves the real emulator (the harness lives in
//! `repository_lifecycle`, which this suite shares rather than rebuilding).
//!
//! The rules under test:
//!
//! 1. A successful update REPLACES the stored template's content with the
//!    `newRuleContent` sent, byte for byte, and changes nothing else a caller can
//!    name: the template's id, name, description and `creationDate` are the ones the
//!    create answered, while `ruleContentSha256` is the digest of the content now
//!    stored, `lastModifiedUser` is the ARN of the caller that made THIS change and
//!    `lastModifiedDate` is no earlier than the creation date. What the update
//!    answers is what `GetApprovalRuleTemplate` reads back, so the change is in
//!    stored state rather than in one response.
//! 2. `existingRuleContentSha256` is a precondition on the content being replaced:
//!    the update applies when it is the digest of the stored content (in either hex
//!    case) and is refused with `InvalidRuleContentSha256Exception` for every other
//!    value — a stale digest, the digest of the new content, or a string that is not
//!    a SHA-256 digest at all — leaving the stored template exactly as it was.
//! 3. Content that is not a complete approval rule document, and a template name no
//!    template is stored under, are refused with the error this operation's model
//!    declares for that cause, at the status the model gives it, and the stored
//!    template is preserved.
//!
//! Each table spans the class its rule covers — document shape and size, the
//! presence and case of the guard digest, the ways a document can fail the schema and
//! the ways a name can name no template — rather than the one recorded request.

use serde_json::{json, Value};
use sha2::Digest;

use crate::repository_lifecycle::{emulator, string_at};
use crate::wire::{caller_arn, call_as, error_as, ok_as};

/// The `Version` and statement `Type` CodeCommit's approval rule template schema
/// defines (the structure the CodeCommit User Guide documents for approval rule
/// template content, as `src/state.rs`'s content validator reads it).
const TEMPLATE_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// The caller that creates each template, and the (different) caller that updates
/// it: `lastModifiedUser` must name the one that made the most recent change, so the
/// two cannot be the same principal.
const AUTHOR_KEY_ID: &str = "AKIDTEMPLATEAUTHOR";
const EDITOR_KEY_ID: &str = "AKIDTEMPLATEEDITOR";

/// An approval rule document with `approvals` approvals needed and the given pool.
fn document(approvals: i64, pool: &[&str]) -> String {
    let mut statement = json!({
        "Type": STATEMENT_TYPE,
        "NumberOfApprovalsNeeded": approvals,
    });
    if !pool.is_empty() {
        statement["ApprovalPoolMembers"] = json!(pool);
    }
    json!({ "Version": TEMPLATE_VERSION, "Statements": [statement] }).to_string()
}

/// The digest `ruleContentSha256` reports — computed here independently of the
/// service, so the signature is checked against the rule rather than against itself.
fn content_sha256(content: &str) -> String {
    hex::encode(sha2::Sha256::digest(content.as_bytes()))
}

/// A document padded with pool members to exactly `total` characters, for the
/// modelled `ApprovalRuleTemplateContent` bound of 1..3000.
fn padded_document(total: usize) -> String {
    let prefix = format!(
        "{{\"Version\":\"{TEMPLATE_VERSION}\",\"Statements\":[{{\"Type\":\"{STATEMENT_TYPE}\",\
         \"NumberOfApprovalsNeeded\":1,\"ApprovalPoolMembers\":[\""
    );
    let suffix = "\"]}]}";
    let pad = total - prefix.len() - suffix.len();
    format!("{prefix}{}{suffix}", "p".repeat(pad))
}

/// How a row supplies `existingRuleContentSha256`.
#[derive(Debug, Clone, Copy)]
enum Guard {
    /// The member is omitted: the update is unconditional.
    Absent,
    /// The digest of the content currently stored, as `ruleContentSha256` reports it.
    Stored,
    /// That same digest in upper case — hex is case-insensitive, so it names the
    /// same signature.
    StoredUpperCase,
}

/// One row of the update table: the document a template is created with, the
/// document that replaces it, and how the precondition is supplied.
#[derive(Debug, Clone)]
struct UpdateCase {
    label: &'static str,
    created: String,
    replacement: String,
    guard: Guard,
}

/// The table. Rows differ along what the rule generalises over: the shape of the
/// replacement document (approvals needed, pool size, several statements, pretty
/// printing), its size (minimal and the modelled 3000-character maximum), whether it
/// differs from the stored document at all, and whether the precondition is omitted,
/// supplied, or supplied in the other hex case.
fn update_cases() -> Vec<UpdateCase> {
    let several_statements = json!({
        "Version": TEMPLATE_VERSION,
        "Statements": [
            { "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 1 },
            {
                "Type": STATEMENT_TYPE,
                "NumberOfApprovalsNeeded": 4,
                "ApprovalPoolMembers": ["arn:aws:sts::111122223333:assumed-role/Release/*"],
            },
        ],
    })
    .to_string();
    let pretty = serde_json::to_string_pretty(&json!({
        "Version": TEMPLATE_VERSION,
        "DestinationReferences": ["refs/heads/main", "refs/heads/release"],
        "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 3 }],
    }))
    .expect("the document serializes");
    vec![
        UpdateCase {
            label: "more approvals needed, no precondition",
            created: document(1, &[]),
            replacement: document(2, &[]),
            guard: Guard::Absent,
        },
        UpdateCase {
            label: "an approval pool added, guarded by the stored digest",
            created: document(1, &[]),
            replacement: document(
                2,
                &[
                    "arn:aws:iam::111122223333:user/reviewer",
                    "arn:aws:sts::111122223333:assumed-role/Review/*",
                ],
            ),
            guard: Guard::Stored,
        },
        UpdateCase {
            label: "an approval pool removed, guarded by the digest in upper case",
            created: document(3, &["arn:aws:iam::111122223333:user/reviewer"]),
            replacement: document(3, &[]),
            guard: Guard::StoredUpperCase,
        },
        UpdateCase {
            label: "one statement replaced by several",
            created: document(1, &[]),
            replacement: several_statements,
            guard: Guard::Stored,
        },
        UpdateCase {
            label: "the same document written as pretty JSON with destinations",
            created: document(3, &[]),
            replacement: pretty,
            guard: Guard::Absent,
        },
        UpdateCase {
            label: "content at the modelled 3000-character maximum",
            created: document(1, &[]),
            replacement: padded_document(3000),
            guard: Guard::Stored,
        },
        UpdateCase {
            label: "the stored document unchanged",
            created: document(2, &["arn:aws:iam::111122223333:user/one"]),
            replacement: document(2, &["arn:aws:iam::111122223333:user/one"]),
            guard: Guard::Stored,
        },
    ]
}

/// The epoch-seconds timestamp awsJson1_1 carries at `pointer`.
fn timestamp_at(payload: &Value, pointer: &str, case: &str) -> f64 {
    payload
        .pointer(pointer)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp at {pointer} in {payload}"))
}

#[tokio::test]
async fn an_update_replaces_only_the_content_and_is_read_back_from_state() {
    let router = emulator();

    for (index, case) in update_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("content-update-{index}");
        let description = format!("the template for case {index}");

        let created = ok_as(
            &router,
            AUTHOR_KEY_ID,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": name,
                "approvalRuleTemplateContent": case.created,
                "approvalRuleTemplateDescription": description,
            }),
        )
        .await;
        let created = created
            .get("approvalRuleTemplate")
            .unwrap_or_else(|| panic!("case {label:?}: no approvalRuleTemplate in the create"))
            .clone();

        let mut request = json!({
            "approvalRuleTemplateName": name,
            "newRuleContent": case.replacement,
        });
        match case.guard {
            Guard::Absent => {}
            Guard::Stored => {
                request["existingRuleContentSha256"] = json!(content_sha256(&case.created));
            }
            Guard::StoredUpperCase => {
                request["existingRuleContentSha256"] =
                    json!(content_sha256(&case.created).to_uppercase());
            }
        }
        let answered = ok_as(
            &router,
            EDITOR_KEY_ID,
            "UpdateApprovalRuleTemplateContent",
            &request,
        )
        .await;
        let updated = answered
            .get("approvalRuleTemplate")
            .unwrap_or_else(|| panic!("case {label:?}: no approvalRuleTemplate in {answered}"))
            .clone();

        // The content is what was sent, byte for byte, and the signature is its
        // digest rather than the one the create reported.
        assert_eq!(
            string_at(&updated, "/approvalRuleTemplateContent"),
            case.replacement,
            "case {label:?}: the content is the replacement, got {updated}"
        );
        assert_eq!(
            string_at(&updated, "/ruleContentSha256"),
            content_sha256(&case.replacement),
            "case {label:?}: ruleContentSha256 is the digest of the stored content, got {updated}"
        );

        // Everything else a caller can name is the template it was: the id, the
        // name, the description and the creation date.
        for (member, pointer) in [
            ("the id", "/approvalRuleTemplateId"),
            ("the name", "/approvalRuleTemplateName"),
            ("the description", "/approvalRuleTemplateDescription"),
        ] {
            assert_eq!(
                updated.pointer(pointer),
                created.pointer(pointer),
                "case {label:?}: {member} is untouched by a content update, got {updated}"
            );
        }
        let creation = timestamp_at(&updated, "/creationDate", label);
        assert_eq!(
            creation,
            timestamp_at(&created, "/creationDate", label),
            "case {label:?}: the creation date is untouched, got {updated}"
        );

        // The change is attributed to the caller that made it, at an instant no
        // earlier than the creation.
        assert_eq!(
            string_at(&updated, "/lastModifiedUser"),
            caller_arn(EDITOR_KEY_ID),
            "case {label:?}: lastModifiedUser is the caller that updated, got {updated}"
        );
        let modified = timestamp_at(&updated, "/lastModifiedDate", label);
        assert!(
            modified >= creation,
            "case {label:?}: lastModifiedDate {modified} must be at or after the creation \
             date {creation}"
        );

        // And the update is in STORED state: the reader answers the same template.
        let fetched = ok_as(
            &router,
            AUTHOR_KEY_ID,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": name }),
        )
        .await;
        assert_eq!(
            fetched.get("approvalRuleTemplate"),
            Some(&updated),
            "case {label:?}: the stored template must be exactly what the update answered"
        );
    }
}

#[tokio::test]
async fn successive_updates_each_replace_the_content_the_previous_one_left() {
    let router = emulator();
    let name = "chained-updates";
    let first = document(1, &[]);
    ok_as(
        &router,
        AUTHOR_KEY_ID,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": name,
            "approvalRuleTemplateContent": first,
        }),
    )
    .await;

    // Each step guards itself with the signature the PREVIOUS answer reported, which
    // is the round trip a caller makes: read the signature, send the next content.
    let mut expected = first;
    let mut guard = content_sha256(&expected);
    for approvals in [2i64, 5, 1, 7] {
        let replacement = document(approvals, &["arn:aws:iam::111122223333:user/one"]);
        let answered = ok_as(
            &router,
            EDITOR_KEY_ID,
            "UpdateApprovalRuleTemplateContent",
            &json!({
                "approvalRuleTemplateName": name,
                "newRuleContent": replacement,
                "existingRuleContentSha256": guard,
            }),
        )
        .await;
        let reported = string_at(&answered, "/approvalRuleTemplate/ruleContentSha256");
        assert_eq!(
            string_at(&answered, "/approvalRuleTemplate/approvalRuleTemplateContent"),
            replacement,
            "case {approvals} approvals: the content is the one just sent, got {answered}"
        );
        assert_eq!(
            reported,
            content_sha256(&replacement),
            "case {approvals} approvals: the signature is the digest of that content"
        );

        // The signature the previous step reported is now stale: the step that
        // already consumed it cannot be replayed.
        let stale = content_sha256(&expected);
        let code = error_as(
            &router,
            EDITOR_KEY_ID,
            "UpdateApprovalRuleTemplateContent",
            &json!({
                "approvalRuleTemplateName": name,
                "newRuleContent": document(9, &[]),
                "existingRuleContentSha256": stale,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidRuleContentSha256Exception",
            "case {approvals} approvals: the digest of the replaced content is stale"
        );

        expected = replacement;
        guard = reported;
    }

    let fetched = ok_as(
        &router,
        AUTHOR_KEY_ID,
        "GetApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": name }),
    )
    .await;
    assert_eq!(
        string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateContent"),
        expected,
        "the stored content is the last one written, got {fetched}"
    );
}

#[tokio::test]
async fn a_precondition_that_is_not_the_stored_signature_is_refused_and_nothing_changes() {
    let router = emulator();
    let name = "guarded-template";
    let stored = document(2, &["arn:aws:iam::111122223333:user/one"]);
    ok_as(
        &router,
        AUTHOR_KEY_ID,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": name,
            "approvalRuleTemplateContent": stored,
        }),
    )
    .await;

    let replacement = document(4, &[]);
    let digest = content_sha256(&stored);
    // Rows differ along the ways a supplied signature can fail to be the stored
    // content's: the digest of another document, the digest of the replacement, a
    // digest of the right form that no content has, one hex character short or long,
    // a non-hex string of the right length, prose, and an empty member.
    let cases: [(&str, String); 8] = [
        ("the digest of a different document", content_sha256(&document(1, &[]))),
        ("the digest of the new content", content_sha256(&replacement)),
        ("a well-formed digest of nothing stored", "a".repeat(64)),
        ("one character short", digest[..63].to_string()),
        ("one character too long", format!("{digest}0")),
        ("the right length but not hex", "z".repeat(64)),
        ("prose", "not-a-signature".to_string()),
        ("an empty signature", String::new()),
    ];

    for (label, supplied) in cases {
        let code = error_as(
            &router,
            EDITOR_KEY_ID,
            "UpdateApprovalRuleTemplateContent",
            &json!({
                "approvalRuleTemplateName": name,
                "newRuleContent": replacement,
                "existingRuleContentSha256": supplied,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidRuleContentSha256Exception",
            "case {label:?}: {supplied:?} is not the signature of the stored content"
        );

        let fetched = ok_as(
            &router,
            AUTHOR_KEY_ID,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": name }),
        )
        .await;
        assert_eq!(
            string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateContent"),
            stored,
            "case {label:?}: the refused update must leave the stored content, got {fetched}"
        );
        assert_eq!(
            string_at(&fetched, "/approvalRuleTemplate/lastModifiedUser"),
            caller_arn(AUTHOR_KEY_ID),
            "case {label:?}: a refused update attributes nothing, got {fetched}"
        );
    }
}

#[tokio::test]
async fn content_that_is_not_a_complete_approval_rule_document_is_refused() {
    let router = emulator();
    let name = "preserved-template";
    let stored = document(2, &[]);
    ok_as(
        &router,
        AUTHOR_KEY_ID,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": name,
            "approvalRuleTemplateContent": stored,
        }),
    )
    .await;

    let invalid = "InvalidApprovalRuleTemplateContentException";
    // Rows differ along the ways a document can fail the approval rule schema — not
    // JSON at all, not an object, a `Version` the schema does not define, no or empty
    // `Statements`, a statement `Type` it does not define, a `NumberOfApprovalsNeeded`
    // below one or not a number, a pool of the wrong type — plus content that is
    // nothing but whitespace, which is EMPTY content rather than invalid content.
    let cases: [(&str, String, &str); 11] = [
        ("prose that is not JSON", "replace the rule please".to_string(), invalid),
        ("a JSON array", "[]".to_string(), invalid),
        (
            "a Version the schema does not define",
            json!({
                "Version": "2525-01-01",
                "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 1 }],
            })
            .to_string(),
            invalid,
        ),
        (
            "no Statements",
            json!({ "Version": TEMPLATE_VERSION }).to_string(),
            invalid,
        ),
        (
            "an empty Statements array",
            json!({ "Version": TEMPLATE_VERSION, "Statements": [] }).to_string(),
            invalid,
        ),
        (
            "a statement Type the schema does not define",
            json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{ "Type": "Blockers", "NumberOfApprovalsNeeded": 1 }],
            })
            .to_string(),
            invalid,
        ),
        (
            "NumberOfApprovalsNeeded of zero",
            json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 0 }],
            })
            .to_string(),
            invalid,
        ),
        (
            "NumberOfApprovalsNeeded as prose",
            json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": "four" }],
            })
            .to_string(),
            invalid,
        ),
        (
            "an ApprovalPoolMembers entry that is not a string",
            json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{
                    "Type": STATEMENT_TYPE,
                    "NumberOfApprovalsNeeded": 1,
                    "ApprovalPoolMembers": [7],
                }],
            })
            .to_string(),
            invalid,
        ),
        (
            "content of nothing but whitespace",
            "  \n\t".to_string(),
            "ApprovalRuleTemplateContentRequiredException",
        ),
        (
            "a single space",
            " ".to_string(),
            "ApprovalRuleTemplateContentRequiredException",
        ),
    ];

    for (label, content, expected_code) in cases {
        let code = error_as(
            &router,
            EDITOR_KEY_ID,
            "UpdateApprovalRuleTemplateContent",
            &json!({
                "approvalRuleTemplateName": name,
                "newRuleContent": content,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, expected_code,
            "case {label:?}: expected {expected_code}, got {code}"
        );

        let fetched = ok_as(
            &router,
            AUTHOR_KEY_ID,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": name }),
        )
        .await;
        assert_eq!(
            string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateContent"),
            stored,
            "case {label:?}: the refused update must leave the stored content, got {fetched}"
        );
    }

    // One character past the modelled `ApprovalRuleTemplateContent` maximum of 3000
    // is refused by the boundary validator before the handler runs, so it is a 400
    // with no template touched. The error CODE is not pinned here: the refusal
    // renderer sees only the member name `newRuleContent`, which
    // `UpdatePullRequestApprovalRuleContent` also uses for a differently modelled
    // member, so this operation's name for that refusal is not resolvable there.
    let (status, payload) = call_as(
        &router,
        EDITOR_KEY_ID,
        "UpdateApprovalRuleTemplateContent",
        &json!({
            "approvalRuleTemplateName": name,
            "newRuleContent": padded_document(3001),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"content past the modelled maximum\": expected a 400, got {status} {payload}"
    );
    let fetched = ok_as(
        &router,
        AUTHOR_KEY_ID,
        "GetApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": name }),
    )
    .await;
    assert_eq!(
        string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateContent"),
        stored,
        "case \"content past the modelled maximum\": the stored content is preserved"
    );
}

#[tokio::test]
async fn a_name_that_names_no_template_is_refused_with_the_modeled_error() {
    let router = emulator();
    let existing = "cased-template";
    ok_as(
        &router,
        AUTHOR_KEY_ID,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": existing,
            "approvalRuleTemplateContent": document(1, &[]),
        }),
    )
    .await;

    let replacement = document(2, &[]);
    // Rows differ along the ways a name can name no stored template: never created,
    // a name differing only in case from one that exists (template names are
    // case-sensitive keys), a name at the modelled 100-character maximum, a name of
    // nothing but whitespace (not a valid name at all), and an absent or empty name
    // (no name was specified).
    let cases: [(&str, Option<String>, &str); 6] = [
        (
            "a name never created",
            Some("no-such-template".to_string()),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "the stored name in another case",
            Some(existing.to_uppercase()),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "a name at the modelled maximum length",
            Some("n".repeat(100)),
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "a name of nothing but whitespace",
            Some("   ".to_string()),
            "InvalidApprovalRuleTemplateNameException",
        ),
        (
            "an empty name",
            Some(String::new()),
            "ApprovalRuleTemplateNameRequiredException",
        ),
        (
            "no name member",
            None,
            "ApprovalRuleTemplateNameRequiredException",
        ),
    ];

    for (label, name, expected_code) in cases {
        let mut request = json!({ "newRuleContent": replacement });
        if let Some(name) = &name {
            request["approvalRuleTemplateName"] = json!(name);
        }
        let code = error_as(
            &router,
            EDITOR_KEY_ID,
            "UpdateApprovalRuleTemplateContent",
            &request,
            400,
        )
        .await;
        assert_eq!(
            code, expected_code,
            "case {label:?}: expected {expected_code}, got {code}"
        );
    }

    // No refusal created a template, and the one that existed is unchanged.
    let listed = ok_as(
        &router,
        AUTHOR_KEY_ID,
        "ListApprovalRuleTemplates",
        &json!({}),
    )
    .await;
    assert_eq!(
        listed.pointer("/approvalRuleTemplateNames"),
        Some(&json!([existing])),
        "the refusals must leave exactly the created template, got {listed}"
    );
    let fetched = ok_as(
        &router,
        AUTHOR_KEY_ID,
        "GetApprovalRuleTemplate",
        &json!({ "approvalRuleTemplateName": existing }),
    )
    .await;
    assert_eq!(
        string_at(&fetched, "/approvalRuleTemplate/approvalRuleTemplateContent"),
        document(1, &[]),
        "the stored template is the one created, got {fetched}"
    );
}
