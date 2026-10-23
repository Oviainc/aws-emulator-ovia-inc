// SPDX-License-Identifier: CC0-1.0

//! Approval rule templates and commits over the wire: `CreateApprovalRuleTemplate`,
//! `GetApprovalRuleTemplate`, `ListApprovalRuleTemplates`,
//! `AssociateApprovalRuleTemplateWithRepository`,
//! `ListAssociatedApprovalRuleTemplatesForRepository`, `CreateCommit` and
//! `GetCommit`, driven as awsJson1_1 requests through the router that serves the
//! real emulator (the harness lives in `repository_lifecycle`, which this suite
//! shares rather than rebuilding).
//!
//! The rules under test:
//!
//! * a request the service cannot serve is answered with the error the operation's
//!   model declares for that cause, at the status the model gives it, and the store
//!   is left exactly as it was — never with a 501 that says "not written yet";
//! * every paginated list operation refuses a `maxResults` outside the published
//!   1..=1000 range with `InvalidMaxResultsException` and a `nextToken` it did not
//!   issue with `InvalidContinuationTokenException`, before cutting any page, and
//!   otherwise answers a page of at most `maxResults` names plus a token only when a
//!   further page exists;
//! * a write is computed into stored state and read back through the implemented
//!   reader: a template's projection is the same from the create and the get, and a
//!   commit's tree, parents and identity are what `GetCommit` and `GetFile` report.
//!
//! Each table spans the class its rule covers — name and description lengths,
//! content shapes, page sizes and tokens, file entry kinds, id forms — rather than
//! one recorded request.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use sha2::Digest;
use smithy_server_runtime::MultiServiceRouter;

use super::repository_lifecycle::{
    base64, call, emulator, error, from_base64, is_object_id, is_uuid, ok, string_at,
};

/// The schema version of an approval rule template document, and the only statement
/// type the schema defines. Both are from the approval rule template content the
/// CodeCommit User Guide documents (and `src/state.rs`'s
/// `APPROVAL_RULE_TEMPLATE_VERSION` / `APPROVAL_RULE_STATEMENT_TYPE`).
const TEMPLATE_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// A valid approval rule template document, as compact JSON text.
fn template_content(approvals: i64, pool: &[&str], destinations: &[&str]) -> String {
    let mut document = json!({
        "Version": TEMPLATE_VERSION,
        "Statements": [{
            "Type": STATEMENT_TYPE,
            "NumberOfApprovalsNeeded": approvals,
            "ApprovalPoolMembers": pool,
        }],
    });
    if !destinations.is_empty() {
        document["DestinationReferences"] = json!(destinations);
    }
    document.to_string()
}

/// The simplest valid document: one approver needed, no pool, no destinations.
fn minimal_content() -> String {
    json!({
        "Version": TEMPLATE_VERSION,
        "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 1 }],
    })
    .to_string()
}

/// The SHA-256 hash signature of a template's content, computed here independently
/// of the service, so `ruleContentSha256` is checked against the rule rather than
/// against itself.
fn content_sha256(content: &str) -> String {
    hex::encode(sha2::Sha256::digest(content.as_bytes()))
}

/// The names a list operation answered.
fn names_of(payload: &Value, case: &str) -> Vec<String> {
    payload
        .get("approvalRuleTemplateNames")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no approvalRuleTemplateNames in {payload}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a name is not a string in {payload}"))
                .to_string()
        })
        .collect()
}

/// A 40-hex object name that is not the id of anything: the digest of `seed`,
/// truncated the way a git object name is.
fn absent_object_id(seed: &str) -> String {
    hex::encode(&sha2::Sha256::digest(seed.as_bytes())[..20])
}

// ---------------------------------------------------------------------------
// 1. Pagination
// ---------------------------------------------------------------------------

/// One row of the pagination table: what the request asks for, and the error code
/// the service must answer with (`None` when the request must succeed).
#[derive(Debug, Clone)]
struct PageCase {
    label: &'static str,
    max_results: Option<i64>,
    next_token: Option<&'static str>,
    expected_code: Option<&'static str>,
}

/// The table. Rows differ along what the rule generalises over: whether each
/// pagination member is present at all, and — for `maxResults` — both published
/// bounds and one past each, and for `nextToken` the forms a token can take. The
/// range is CodeCommit's published valid limit for `maxResults` ("A valid limit is
/// between 1 and 1000"). A token is opaque, so a value the service did not issue —
/// including a bare offset into the sequence — is not one, and following an issued
/// token is covered by the paging test below.
fn page_cases() -> Vec<PageCase> {
    vec![
        PageCase {
            label: "neither member: the positive control",
            max_results: None,
            next_token: None,
            expected_code: None,
        },
        PageCase {
            label: "maxResults at the published minimum",
            max_results: Some(1),
            next_token: None,
            expected_code: None,
        },
        PageCase {
            label: "maxResults at the published maximum",
            max_results: Some(1000),
            next_token: None,
            expected_code: None,
        },
        PageCase {
            label: "maxResults one past the minimum",
            max_results: Some(0),
            next_token: None,
            expected_code: Some("InvalidMaxResultsException"),
        },
        PageCase {
            label: "maxResults negative",
            max_results: Some(-1),
            next_token: None,
            expected_code: Some("InvalidMaxResultsException"),
        },
        PageCase {
            label: "maxResults at the integer floor",
            max_results: Some(i64::from(i32::MIN)),
            next_token: None,
            expected_code: Some("InvalidMaxResultsException"),
        },
        PageCase {
            label: "maxResults one past the maximum",
            max_results: Some(1001),
            next_token: None,
            expected_code: Some("InvalidMaxResultsException"),
        },
        PageCase {
            label: "maxResults at the integer ceiling",
            max_results: Some(i64::from(i32::MAX)),
            next_token: None,
            expected_code: Some("InvalidMaxResultsException"),
        },
        PageCase {
            label: "nextToken of a bare offset the service never issued",
            max_results: None,
            next_token: Some("0"),
            expected_code: Some("InvalidContinuationTokenException"),
        },
        PageCase {
            label: "nextToken of a bare offset inside the sequence",
            max_results: None,
            next_token: Some("2"),
            expected_code: Some("InvalidContinuationTokenException"),
        },
        PageCase {
            label: "nextToken of prose",
            max_results: None,
            next_token: Some("not-a-real-continuation-token"),
            expected_code: Some("InvalidContinuationTokenException"),
        },
        PageCase {
            label: "nextToken empty",
            max_results: None,
            next_token: Some(""),
            expected_code: Some("InvalidContinuationTokenException"),
        },
        PageCase {
            label: "nextToken negative",
            max_results: None,
            next_token: Some("-1"),
            expected_code: Some("InvalidContinuationTokenException"),
        },
        PageCase {
            label: "nextToken of a bare offset inside the sequence",
            max_results: None,
            next_token: Some("2"),
            expected_code: Some("InvalidContinuationTokenException"),
        },
        PageCase {
            label: "nextToken in scientific notation",
            max_results: None,
            next_token: Some("1e3"),
            expected_code: Some("InvalidContinuationTokenException"),
        },
        PageCase {
            label: "a bad maxResults and a bad nextToken together",
            max_results: Some(0),
            next_token: Some("not-a-real-continuation-token"),
            expected_code: Some("InvalidMaxResultsException"),
        },
    ]
}

/// The repository every association in this suite's paging test is made against.
const PAGING_REPOSITORY: &str = "paging-repo";

/// Three templates, each associated with `PAGING_REPOSITORY`, so both paginated
/// operations answer the same sequence of names.
async fn seeded_paging_emulator() -> (MultiServiceRouter, Vec<String>) {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": PAGING_REPOSITORY }),
    )
    .await;
    let names: Vec<String> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for name in &names {
        ok(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": name,
                "approvalRuleTemplateContent": minimal_content(),
            }),
        )
        .await;
        ok(
            &router,
            "AssociateApprovalRuleTemplateWithRepository",
            &json!({
                "approvalRuleTemplateName": name,
                "repositoryName": PAGING_REPOSITORY,
            }),
        )
        .await;
    }
    (router, names)
}

/// The request body for one paginated list operation and one row of the table.
fn page_body(operation: &str, case: &PageCase) -> Value {
    let mut body = json!({});
    if operation == "ListAssociatedApprovalRuleTemplatesForRepository" {
        body["repositoryName"] = json!(PAGING_REPOSITORY);
    }
    if let Some(max_results) = case.max_results {
        body["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = case.next_token {
        body["nextToken"] = json!(next_token);
    }
    body
}

#[tokio::test]
async fn pagination_members_are_refused_with_the_error_the_model_declares() {
    let (router, all_names) = seeded_paging_emulator().await;

    for operation in [
        "ListApprovalRuleTemplates",
        "ListAssociatedApprovalRuleTemplatesForRepository",
    ] {
        for case in page_cases() {
            let body = page_body(operation, &case);
            let label = format!("{operation}: {}", case.label);
            match case.expected_code {
                Some(expected) => {
                    let code = error(&router, operation, &body, 400).await;
                    assert_eq!(
                        code, expected,
                        "case {label:?}: expected {expected}, got {code:?}"
                    );
                }
                None => {
                    let payload = ok(&router, operation, &body).await;
                    let answered = names_of(&payload, &label);
                    let limit = case
                        .max_results
                        .map_or(all_names.len(), |max| usize::try_from(max).unwrap_or(0));
                    assert!(
                        answered.len() <= limit,
                        "case {label:?}: a page carries at most maxResults names, got {answered:?}"
                    );
                    assert!(
                        answered.iter().all(|name| all_names.contains(name)),
                        "case {label:?}: every answered name is a stored template, got {answered:?}"
                    );
                }
            }
        }
    }

    // A refused page changes nothing: the whole sequence is still answerable after
    // every row above.
    for operation in [
        "ListApprovalRuleTemplates",
        "ListAssociatedApprovalRuleTemplatesForRepository",
    ] {
        let body = page_body(
            operation,
            &PageCase {
                label: "after the table",
                max_results: None,
                next_token: None,
                expected_code: None,
            },
        );
        let payload = ok(&router, operation, &body).await;
        assert_eq!(
            names_of(&payload, operation),
            all_names,
            "case {operation:?}: the full listing must still answer every stored name"
        );
        assert!(
            payload.get("nextToken").is_none_or(Value::is_null),
            "case {operation:?}: a listing that returned everything carries no token, got {payload}"
        );
    }

    // A token belongs to the listing that issued it: the token
    // `ListApprovalRuleTemplates` hands out is not a token for the per-repository
    // listing, nor for the same listing of another repository.
    let issued = ok(
        &router,
        "ListApprovalRuleTemplates",
        &json!({ "maxResults": 1 }),
    )
    .await;
    let token = string_at(&issued, "/nextToken");
    let code = error(
        &router,
        "ListAssociatedApprovalRuleTemplatesForRepository",
        &json!({ "repositoryName": PAGING_REPOSITORY, "nextToken": token }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidContinuationTokenException",
        "case \"a token issued by another listing\": expected \
         InvalidContinuationTokenException, got {code:?}"
    );

    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "paging-repo-two" }),
    )
    .await;
    for name in &all_names {
        ok(
            &router,
            "AssociateApprovalRuleTemplateWithRepository",
            &json!({ "approvalRuleTemplateName": name, "repositoryName": "paging-repo-two" }),
        )
        .await;
    }
    let issued = ok(
        &router,
        "ListAssociatedApprovalRuleTemplatesForRepository",
        &json!({ "repositoryName": PAGING_REPOSITORY, "maxResults": 1 }),
    )
    .await;
    let token = string_at(&issued, "/nextToken");
    let code = error(
        &router,
        "ListAssociatedApprovalRuleTemplatesForRepository",
        &json!({ "repositoryName": "paging-repo-two", "nextToken": token }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidContinuationTokenException",
        "case \"a token issued for another repository's listing\": expected \
         InvalidContinuationTokenException, got {code:?}"
    );
}

#[tokio::test]
async fn a_page_carries_max_results_names_and_a_token_only_when_more_remain() {
    let (router, all_names) = seeded_paging_emulator().await;

    for operation in [
        "ListApprovalRuleTemplates",
        "ListAssociatedApprovalRuleTemplatesForRepository",
    ] {
        // Rows: every page size from one name to one more than the sequence holds.
        for page_size in 1..=(all_names.len() + 1) {
            let label = format!("{operation}: pages of {page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut body = json!({ "maxResults": page_size });
                if operation == "ListAssociatedApprovalRuleTemplatesForRepository" {
                    body["repositoryName"] = json!(PAGING_REPOSITORY);
                }
                if let Some(token) = &token {
                    body["nextToken"] = json!(token);
                }
                let payload = ok(&router, operation, &body).await;
                let page = names_of(&payload, &label);
                assert!(
                    page.len() <= page_size,
                    "case {label:?}: a page carries at most {page_size} names, got {page:?}"
                );
                seen.extend(page);
                pages += 1;
                assert!(
                    pages <= all_names.len() + 1,
                    "case {label:?}: paging must terminate, saw {pages} pages"
                );
                match payload.get("nextToken").and_then(Value::as_str) {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                seen, all_names,
                "case {label:?}: paging must answer every name exactly once, in order"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Creating and reading back an approval rule template
// ---------------------------------------------------------------------------

/// One row of the template table.
#[derive(Debug, Clone)]
struct TemplateCase {
    label: &'static str,
    name: String,
    description: Option<String>,
    /// The content as submitted.
    content: String,
    /// The content the service stores and reports: the submitted document
    /// canonicalized to minified JSON. Equal to `content` for every row written
    /// minified already; the canonicalization itself is pinned by
    /// `tests/all/approval_rule_template_content_canonicalization.rs`.
    stored: String,
}

/// The table. Rows differ along what the rule generalises over: name length (the
/// modeled `ApprovalRuleTemplateName` bounds 1..100), description (absent, empty,
/// the modeled `ApprovalRuleTemplateDescription` maximum of 1000) and content shape
/// (minimal, an approval pool, several statements, destination references, and the
/// same document written as pretty JSON).
fn template_cases() -> Vec<TemplateCase> {
    let many_statements = json!({
        "Version": TEMPLATE_VERSION,
        "Statements": [
            { "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 1 },
            {
                "Type": STATEMENT_TYPE,
                "NumberOfApprovalsNeeded": 3,
                "ApprovalPoolMembers": ["arn:aws:sts::111122223333:assumed-role/Review/*"],
            },
        ],
    })
    .to_string();
    let pretty_document = json!({
        "Version": TEMPLATE_VERSION,
        "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 2 }],
    });
    let pretty =
        serde_json::to_string_pretty(&pretty_document).expect("the document serializes");
    vec![
        TemplateCase {
            label: "a one-character name and the minimal document",
            name: "t".to_string(),
            description: None,
            content: minimal_content(),
            stored: minimal_content(),
        },
        TemplateCase {
            label: "an ordinary name, a description and an approval pool",
            name: "require-two-reviewers".to_string(),
            description: Some("two approvals on the release branch".to_string()),
            content: template_content(
                2,
                &[
                    "arn:aws:iam::111122223333:user/reviewer",
                    "arn:aws:sts::111122223333:assumed-role/Review/*",
                ],
                &["refs/heads/main"],
            ),
            stored: template_content(
                2,
                &[
                    "arn:aws:iam::111122223333:user/reviewer",
                    "arn:aws:sts::111122223333:assumed-role/Review/*",
                ],
                &["refs/heads/main"],
            ),
        },
        TemplateCase {
            label: "the longest name and an empty description",
            name: "n".repeat(100),
            description: Some(String::new()),
            content: template_content(1, &["arn:aws:iam::111122223333:user/one"], &[]),
            stored: template_content(1, &["arn:aws:iam::111122223333:user/one"], &[]),
        },
        TemplateCase {
            label: "the longest description and several statements",
            name: "many-statements".to_string(),
            description: Some("d".repeat(1000)),
            content: many_statements.clone(),
            stored: many_statements,
        },
        TemplateCase {
            label: "the document written as pretty JSON",
            name: "pretty.document-1".to_string(),
            description: None,
            content: pretty,
            // Stored minified: the service canonicalizes the document rather than
            // echoing the submitted indentation.
            stored: pretty_document.to_string(),
        },
    ]
}

#[tokio::test]
async fn a_created_approval_rule_template_is_read_back_from_state() {
    let router = emulator();

    for case in template_cases() {
        let label = case.label;
        let mut request = json!({
            "approvalRuleTemplateName": case.name,
            "approvalRuleTemplateContent": case.content,
        });
        if let Some(description) = &case.description {
            request["approvalRuleTemplateDescription"] = json!(description);
        }
        let created = ok(&router, "CreateApprovalRuleTemplate", &request).await;
        let template = created
            .get("approvalRuleTemplate")
            .unwrap_or_else(|| panic!("case {label:?}: no approvalRuleTemplate in {created}"));

        assert_eq!(
            string_at(template, "/approvalRuleTemplateName"),
            case.name,
            "case {label:?}: the name is the one created"
        );
        assert_eq!(
            string_at(template, "/approvalRuleTemplateContent"),
            case.stored,
            "case {label:?}: the content is stored in its canonical minified form"
        );
        assert_eq!(
            string_at(template, "/ruleContentSha256"),
            content_sha256(&case.stored),
            "case {label:?}: ruleContentSha256 is the digest of the stored content"
        );
        let id = string_at(template, "/approvalRuleTemplateId");
        assert!(
            is_uuid(&id),
            "case {label:?}: the template id is a UUID, got {id:?}"
        );
        let user = string_at(template, "/lastModifiedUser");
        assert!(
            user.starts_with("arn:aws:iam::"),
            "case {label:?}: lastModifiedUser is an IAM ARN, got {user:?}"
        );
        assert_eq!(
            template.get("approvalRuleTemplateDescription").and_then(Value::as_str),
            // An empty description is held as NO description: CodeCommit reports
            // `approvalRuleTemplateDescription` absent rather than as an empty
            // string (the recorded behaviour the corpus names
            // "approval-rule-template-empty-description-normalized-to-absent").
            case.description.as_deref().filter(|text| !text.is_empty()),
            "case {label:?}: the description is the one created"
        );
        for member in ["creationDate", "lastModifiedDate"] {
            assert!(
                template.get(member).and_then(Value::as_f64).is_some(),
                "case {label:?}: {member} is a timestamp, got {template}"
            );
        }

        // The read-back path reports the SAME projection, member for member.
        let fetched = ok(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": case.name }),
        )
        .await;
        assert_eq!(
            fetched.get("approvalRuleTemplate"),
            Some(template),
            "case {label:?}: the get must report what the create stored"
        );
        // Reading it changes nothing a later caller sees.
        let again = ok(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": case.name }),
        )
        .await;
        assert_eq!(
            again, fetched,
            "case {label:?}: a read must not change what a later caller sees"
        );

        // A second create under the same name is refused, and the stored template
        // keeps the content it was created with.
        let code = error(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": case.name,
                "approvalRuleTemplateContent": minimal_content(),
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "ApprovalRuleTemplateNameAlreadyExistsException",
            "case {label:?}: a duplicate name is refused, got {code:?}"
        );
        let after = ok(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": case.name }),
        )
        .await;
        assert_eq!(
            after, fetched,
            "case {label:?}: a refused create leaves the stored template untouched"
        );
    }

    // Every created template is listed, and the listing holds nothing else.
    let listed = ok(&router, "ListApprovalRuleTemplates", &json!({})).await;
    let answered: BTreeSet<String> = names_of(&listed, "every created template")
        .into_iter()
        .collect();
    let expected: BTreeSet<String> = template_cases().into_iter().map(|case| case.name).collect();
    assert_eq!(
        answered, expected,
        "the listing must answer exactly the created templates"
    );
}

/// One row of the malformed-content table: the content sent, and the error code the
/// service must answer with.
#[derive(Debug, Clone)]
struct BadContentCase {
    label: &'static str,
    content: String,
    expected_code: &'static str,
}

/// The table. Rows differ along the way a document can fail the approval rule
/// schema: not JSON at all, JSON that is not an object, a missing or wrong
/// `Version`, missing or empty `Statements`, a statement that is not an object or
/// carries the wrong `Type`, a `NumberOfApprovalsNeeded` that is absent, not a
/// number or below one, and `ApprovalPoolMembers` / `DestinationReferences` of the
/// wrong type — plus content that is nothing but whitespace, which is empty content
/// rather than invalid content.
fn bad_content_cases() -> Vec<BadContentCase> {
    let invalid = "InvalidApprovalRuleTemplateContentException";
    vec![
        BadContentCase {
            label: "prose that is not JSON",
            content: "{ this is not valid approval rule template JSON ".to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "truncated JSON",
            content: "{\"Version\": \"2018-11-08\", \"Statements\": [".to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "a JSON array",
            content: "[]".to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "a JSON string",
            content: "\"Statements\"".to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "an object with no Version",
            content: json!({
                "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 1 }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "a Version the schema does not define",
            content: json!({
                "Version": "2525-01-01",
                "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 1 }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "no Statements",
            content: json!({ "Version": TEMPLATE_VERSION }).to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "an empty Statements array",
            content: json!({ "Version": TEMPLATE_VERSION, "Statements": [] }).to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "a statement that is not an object",
            content: json!({ "Version": TEMPLATE_VERSION, "Statements": ["Approvers"] })
                .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "a statement Type the schema does not define",
            content: json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{ "Type": "Blockers", "NumberOfApprovalsNeeded": 1 }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "a statement with no NumberOfApprovalsNeeded",
            content: json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{ "Type": STATEMENT_TYPE }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "NumberOfApprovalsNeeded of zero",
            content: json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 0 }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "NumberOfApprovalsNeeded negative",
            content: json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": -2 }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "NumberOfApprovalsNeeded as prose",
            content: json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": "two" }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "ApprovalPoolMembers that is not an array",
            content: json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{
                    "Type": STATEMENT_TYPE,
                    "NumberOfApprovalsNeeded": 1,
                    "ApprovalPoolMembers": "arn:aws:iam::111122223333:user/one",
                }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "an ApprovalPoolMembers entry that is not a string",
            content: json!({
                "Version": TEMPLATE_VERSION,
                "Statements": [{
                    "Type": STATEMENT_TYPE,
                    "NumberOfApprovalsNeeded": 1,
                    "ApprovalPoolMembers": [7],
                }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "DestinationReferences that is not an array",
            content: json!({
                "Version": TEMPLATE_VERSION,
                "DestinationReferences": "refs/heads/main",
                "Statements": [{ "Type": STATEMENT_TYPE, "NumberOfApprovalsNeeded": 1 }],
            })
            .to_string(),
            expected_code: invalid,
        },
        BadContentCase {
            label: "content of nothing but whitespace",
            content: "   \n\t".to_string(),
            expected_code: "ApprovalRuleTemplateContentRequiredException",
        },
    ]
}

#[tokio::test]
async fn malformed_template_content_is_refused_and_no_template_is_created() {
    let router = emulator();

    for (index, case) in bad_content_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("bad-{index}");
        let code = error(
            &router,
            "CreateApprovalRuleTemplate",
            &json!({
                "approvalRuleTemplateName": name,
                "approvalRuleTemplateContent": case.content,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, case.expected_code,
            "case {label:?}: expected {}, got {code:?}",
            case.expected_code
        );

        // No template was created: the reader does not know the name.
        let read_back = error(
            &router,
            "GetApprovalRuleTemplate",
            &json!({ "approvalRuleTemplateName": name }),
            400,
        )
        .await;
        assert_eq!(
            read_back, "ApprovalRuleTemplateDoesNotExistException",
            "case {label:?}: a refused create stores nothing, got {read_back:?}"
        );
    }

    let listed = ok(&router, "ListApprovalRuleTemplates", &json!({})).await;
    assert!(
        names_of(&listed, "after every refusal").is_empty(),
        "no refused create may leave a template behind, got {listed}"
    );

    // A name that is nothing but whitespace is not a template name, whatever the
    // content says.
    let code = error(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": "   ",
            "approvalRuleTemplateContent": minimal_content(),
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "InvalidApprovalRuleTemplateNameException",
        "case \"a whitespace-only name\": expected InvalidApprovalRuleTemplateNameException, \
         got {code:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Associating a template with a repository
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_association_needs_both_sides_and_is_read_back_from_state() {
    let router = emulator();
    let repository = "assoc-repo";
    let template = "assoc-template";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    ok(
        &router,
        "CreateApprovalRuleTemplate",
        &json!({
            "approvalRuleTemplateName": template,
            "approvalRuleTemplateContent": minimal_content(),
        }),
    )
    .await;

    // Rows: which side of the association is missing. A name no template carries is
    // refused for the template, whatever the repository, and an existing template
    // against a repository that does not exist is refused for the repository.
    let refusals: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a template name that was never created",
            "ghost",
            repository,
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "a template name that differs only in case",
            "Assoc-Template",
            repository,
            "ApprovalRuleTemplateDoesNotExistException",
        ),
        (
            "a template that exists against a repository that does not",
            template,
            "ghost-repo",
            "RepositoryDoesNotExistException",
        ),
        (
            "neither side exists",
            "ghost",
            "ghost-repo",
            "ApprovalRuleTemplateDoesNotExistException",
        ),
    ];
    for (label, template_name, repository_name, expected) in refusals {
        let code = error(
            &router,
            "AssociateApprovalRuleTemplateWithRepository",
            &json!({
                "approvalRuleTemplateName": template_name,
                "repositoryName": repository_name,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code:?}"
        );
        let listed = ok(
            &router,
            "ListAssociatedApprovalRuleTemplatesForRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
        assert!(
            names_of(&listed, label).is_empty(),
            "case {label:?}: a refused association leaves the repository with none, got {listed}"
        );
    }

    // Both sides exist: the association is stored and read back, and repeating it
    // is the no-op a set makes it.
    for attempt in 1..=2 {
        let label = format!("association attempt {attempt}");
        let (status, payload) = call(
            &router,
            "AssociateApprovalRuleTemplateWithRepository",
            &json!({
                "approvalRuleTemplateName": template,
                "repositoryName": repository,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {payload}"
        );
        let listed = ok(
            &router,
            "ListAssociatedApprovalRuleTemplatesForRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
        assert_eq!(
            names_of(&listed, &label),
            vec![template.to_string()],
            "case {label:?}: the repository carries exactly the associated template"
        );
    }

    // A repository that does not exist has no association listing either.
    let code = error(
        &router,
        "ListAssociatedApprovalRuleTemplatesForRepository",
        &json!({ "repositoryName": "ghost-repo" }),
        400,
    )
    .await;
    assert_eq!(
        code, "RepositoryDoesNotExistException",
        "case \"listing for a repository that does not exist\": expected \
         RepositoryDoesNotExistException, got {code:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. CreateCommit and GetCommit
// ---------------------------------------------------------------------------

/// Create the first commit of a repository, which is what gives it a branch, and
/// answer `(commitId, treeId)`.
async fn seed_repository(router: &MultiServiceRouter, repository: &str, branch: &str) -> String {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let created = ok(
        router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": branch,
            "commitMessage": "the first commit",
            "authorName": "Ada Lovelace",
            "email": "ada@example.com",
            "putFiles": [
                { "filePath": "README.md", "fileContent": base64(b"# demo\n") },
                { "filePath": "src/lib.rs", "fileContent": base64(b"pub fn one() {}\n") },
            ],
        }),
    )
    .await;
    string_at(&created, "/commitId")
}

#[tokio::test]
async fn create_commit_refuses_a_branch_the_repository_does_not_hold() {
    let router = emulator();
    let repository = "commit-branch-repo";
    let tip = seed_repository(&router, repository, "main").await;

    // Rows: names no branch of this repository carries — differing in form, case and
    // whether they look like a ref path — each with a different change to make, so
    // the refusal is about the branch and not about the entries.
    let branches: Vec<(&str, &str, Value)> = vec![
        (
            "a branch that was never created",
            "no-such-branch",
            json!([{ "filePath": "a.txt", "fileContent": base64(b"a") }]),
        ),
        (
            "a branch that differs from the tip's only in case",
            "MAIN",
            json!([{ "filePath": "b/c.txt", "fileContent": base64(b"c") }]),
        ),
        (
            "a fully qualified ref name",
            "refs/heads/main",
            json!([{ "filePath": "d.txt", "fileContent": base64(b"d") }]),
        ),
        (
            "a branch name of one character",
            "x",
            json!([{ "filePath": "e.txt", "fileContent": base64(b"e") }]),
        ),
    ];
    for (label, branch, put_files) in branches {
        let code = error(
            &router,
            "CreateCommit",
            &json!({
                "repositoryName": repository,
                "branchName": branch,
                "putFiles": put_files,
            }),
            400,
        )
        .await;
        assert_eq!(
            code, "BranchDoesNotExistException",
            "case {label:?}: expected BranchDoesNotExistException, got {code:?}"
        );
        // No commit was made: the branch still points at the same tip.
        let branch_read = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": repository, "branchName": "main" }),
        )
        .await;
        assert_eq!(
            string_at(&branch_read, "/branch/commitId"),
            tip,
            "case {label:?}: a refused commit leaves the branch tip where it was"
        );
    }

    // The same request against a repository that does not exist names the
    // repository, not the branch.
    let code = error(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": "ghost-repo",
            "branchName": "main",
            "putFiles": [{ "filePath": "a.txt", "fileContent": base64(b"a") }],
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "RepositoryDoesNotExistException",
        "case \"a repository that does not exist\": expected RepositoryDoesNotExistException, \
         got {code:?}"
    );

    // A commit that names the branch correctly but changes nothing is refused too:
    // a commit must carry at least one entry.
    let code = error(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": tip,
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "FileEntryRequiredException",
        "case \"no file entries\": expected FileEntryRequiredException, got {code:?}"
    );
}

/// One row of the commit table: the change a commit carries, and what the resulting
/// tree must hold.
#[derive(Debug, Clone)]
struct CommitCase {
    label: &'static str,
    /// The request members beyond `repositoryName`, `branchName` and
    /// `parentCommitId`.
    change: Value,
    /// Paths the commit must report under `filesAdded`.
    added: Vec<&'static str>,
    /// Paths the commit must report under `filesDeleted`.
    deleted: Vec<&'static str>,
    /// `(path, content)` the tree must hold at the new commit.
    expected_files: Vec<(&'static str, Vec<u8>)>,
    /// Paths the tree must NOT hold at the new commit.
    absent_files: Vec<&'static str>,
}

/// The table. Rows differ along the kind of change a commit can carry: one put,
/// several puts at different depths, an update of a path that already exists, a
/// delete, a copy and a move through `sourceFile`, a `setFileModes` change, and a
/// delete that empties a folder with `keepEmptyFolders`.
fn commit_cases() -> Vec<CommitCase> {
    vec![
        CommitCase {
            label: "one put at the root",
            change: json!({
                "putFiles": [{ "filePath": "NOTICE", "fileContent": base64(b"notice\n") }],
            }),
            added: vec!["NOTICE"],
            deleted: vec![],
            expected_files: vec![("NOTICE", b"notice\n".to_vec())],
            absent_files: vec![],
        },
        CommitCase {
            label: "several puts at several depths",
            change: json!({
                "putFiles": [
                    { "filePath": "a.txt", "fileContent": base64(b"a") },
                    { "filePath": "one/b.txt", "fileContent": base64(b"b") },
                    {
                        "filePath": "one/two/three/c.bin",
                        "fileContent": base64(&[0u8, 1, 2, 255]),
                        "fileMode": "EXECUTABLE",
                    },
                ],
            }),
            added: vec!["a.txt", "one/b.txt", "one/two/three/c.bin"],
            deleted: vec![],
            expected_files: vec![
                ("a.txt", b"a".to_vec()),
                ("one/b.txt", b"b".to_vec()),
                ("one/two/three/c.bin", vec![0u8, 1, 2, 255]),
            ],
            absent_files: vec![],
        },
        CommitCase {
            label: "an update of a path the tree already holds",
            change: json!({
                "putFiles": [{ "filePath": "README.md", "fileContent": base64(b"# changed\n") }],
            }),
            added: vec![],
            deleted: vec![],
            expected_files: vec![("README.md", b"# changed\n".to_vec())],
            absent_files: vec![],
        },
        CommitCase {
            label: "a delete",
            change: json!({ "deleteFiles": [{ "filePath": "README.md" }] }),
            added: vec![],
            deleted: vec!["README.md"],
            expected_files: vec![("src/lib.rs", b"pub fn one() {}\n".to_vec())],
            absent_files: vec!["README.md"],
        },
        CommitCase {
            label: "a copy through sourceFile",
            change: json!({
                "putFiles": [{
                    "filePath": "docs/README.md",
                    "sourceFile": { "filePath": "README.md" },
                }],
            }),
            added: vec!["docs/README.md"],
            deleted: vec![],
            expected_files: vec![
                ("docs/README.md", b"# demo\n".to_vec()),
                ("README.md", b"# demo\n".to_vec()),
            ],
            absent_files: vec![],
        },
        CommitCase {
            label: "a copy whose source the same commit also updates",
            change: json!({
                "putFiles": [
                    { "filePath": "README.md", "fileContent": base64(b"# changed\n") },
                    {
                        "filePath": "docs/README.md",
                        "sourceFile": { "filePath": "README.md" },
                    },
                ],
            }),
            added: vec!["docs/README.md"],
            deleted: vec![],
            // The copy carries what the BRANCH holds at that path, not what this
            // same request writes over it: `sourceFile` names a file in the branch.
            expected_files: vec![
                ("README.md", b"# changed\n".to_vec()),
                ("docs/README.md", b"# demo\n".to_vec()),
            ],
            absent_files: vec![],
        },
        CommitCase {
            label: "a move through sourceFile",
            change: json!({
                "putFiles": [{
                    "filePath": "docs/README.md",
                    "sourceFile": { "filePath": "README.md", "isMove": true },
                }],
            }),
            added: vec!["docs/README.md"],
            deleted: vec!["README.md"],
            expected_files: vec![("docs/README.md", b"# demo\n".to_vec())],
            absent_files: vec!["README.md"],
        },
        CommitCase {
            label: "a mode change",
            change: json!({
                "setFileModes": [{ "filePath": "src/lib.rs", "fileMode": "EXECUTABLE" }],
            }),
            added: vec![],
            deleted: vec![],
            expected_files: vec![("src/lib.rs", b"pub fn one() {}\n".to_vec())],
            absent_files: vec![],
        },
        CommitCase {
            label: "a delete that empties a folder, keeping the folder",
            change: json!({
                "deleteFiles": [{ "filePath": "src/lib.rs" }],
                "keepEmptyFolders": true,
            }),
            added: vec!["src/.gitkeep"],
            deleted: vec!["src/lib.rs"],
            expected_files: vec![("src/.gitkeep", Vec::new())],
            absent_files: vec!["src/lib.rs"],
        },
        CommitCase {
            label: "a delete that empties a folder, dropping the folder",
            change: json!({ "deleteFiles": [{ "filePath": "src/lib.rs" }] }),
            added: vec![],
            deleted: vec!["src/lib.rs"],
            expected_files: vec![("README.md", b"# demo\n".to_vec())],
            absent_files: vec!["src/lib.rs", "src/.gitkeep"],
        },
    ]
}

/// The `absolutePath` values of one `filesAdded`/`filesUpdated`/`filesDeleted` list.
fn paths_of(payload: &Value, member: &str, case: &str) -> BTreeSet<String> {
    payload
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {payload}"))
        .iter()
        .map(|entry| string_at(entry, "/absolutePath"))
        .collect()
}

#[tokio::test]
async fn a_commit_is_written_to_state_and_read_back_by_get_commit() {
    for case in commit_cases() {
        let label = case.label;
        // One repository per row, so each row starts from the same seeded tree.
        let router = emulator();
        let repository = "commit-repo";
        let parent = seed_repository(&router, repository, "main").await;

        let mut request = json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": parent,
            "commitMessage": label,
            "authorName": "Grace Hopper",
            "email": "grace@example.com",
        });
        for (member, value) in case
            .change
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: the change is an object"))
        {
            request[member] = value.clone();
        }
        let created = ok(&router, "CreateCommit", &request).await;

        let commit_id = string_at(&created, "/commitId");
        let tree_id = string_at(&created, "/treeId");
        assert!(
            is_object_id(&commit_id) && is_object_id(&tree_id),
            "case {label:?}: commitId and treeId are git object names, got \
             {commit_id:?} and {tree_id:?}"
        );
        assert_ne!(
            commit_id, parent,
            "case {label:?}: a commit that changes the tree has its own id"
        );
        assert_eq!(
            paths_of(&created, "filesAdded", label),
            case.added.iter().map(|path| (*path).to_string()).collect(),
            "case {label:?}: filesAdded names the paths this commit added"
        );
        assert_eq!(
            paths_of(&created, "filesDeleted", label),
            case.deleted.iter().map(|path| (*path).to_string()).collect(),
            "case {label:?}: filesDeleted names the paths this commit deleted"
        );

        // GetCommit reports the stored commit: its tree, its parent and the author
        // and committer identity the request carried.
        let fetched = ok(
            &router,
            "GetCommit",
            &json!({ "repositoryName": repository, "commitId": commit_id }),
        )
        .await;
        let commit = fetched
            .get("commit")
            .unwrap_or_else(|| panic!("case {label:?}: no commit in {fetched}"));
        assert_eq!(
            string_at(commit, "/commitId"),
            commit_id,
            "case {label:?}: the commit reports the id it was created with"
        );
        assert_eq!(
            string_at(commit, "/treeId"),
            tree_id,
            "case {label:?}: the commit reports the tree the create answered"
        );
        assert_eq!(
            commit
                .get("parents")
                .and_then(Value::as_array)
                .map(|parents| parents.iter().map(|id| id.as_str().unwrap_or_default().to_string()).collect::<Vec<String>>()),
            Some(vec![parent.clone()]),
            "case {label:?}: the commit's parent is the tip it was written on"
        );
        assert_eq!(
            string_at(commit, "/message"),
            label,
            "case {label:?}: the commit carries the message it was created with"
        );
        for member in ["author", "committer"] {
            let who = commit
                .get(member)
                .unwrap_or_else(|| panic!("case {label:?}: no {member} in {commit}"));
            assert_eq!(
                string_at(who, "/name"),
                "Grace Hopper",
                "case {label:?}: {member} names the author the request gave"
            );
            assert_eq!(
                string_at(who, "/email"),
                "grace@example.com",
                "case {label:?}: {member} carries the email the request gave"
            );
            let date = string_at(who, "/date");
            assert!(
                date.split_once(' ').is_some_and(|(secs, offset)| {
                    secs.parse::<u64>().is_ok() && offset.starts_with('+')
                }),
                "case {label:?}: {member} date is unix seconds plus a GMT offset, got {date:?}"
            );
        }

        // The branch now points at the new commit, and the tree holds what the
        // commit said it would.
        let branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": repository, "branchName": "main" }),
        )
        .await;
        assert_eq!(
            string_at(&branch, "/branch/commitId"),
            commit_id,
            "case {label:?}: the branch tip is the new commit"
        );
        for (path, content) in &case.expected_files {
            let file = ok(
                &router,
                "GetFile",
                &json!({
                    "repositoryName": repository,
                    "commitSpecifier": commit_id,
                    "filePath": path,
                }),
            )
            .await;
            assert_eq!(
                from_base64(&string_at(&file, "/fileContent")),
                *content,
                "case {label:?}: the tree holds {path} with the content the commit wrote"
            );
        }
        for path in &case.absent_files {
            let code = error(
                &router,
                "GetFile",
                &json!({
                    "repositoryName": repository,
                    "commitSpecifier": commit_id,
                    "filePath": path,
                }),
                400,
            )
            .await;
            assert_eq!(
                code, "FileDoesNotExistException",
                "case {label:?}: {path} is not in the tree this commit wrote, got {code:?}"
            );
        }

        // The parent commit is still readable and still reports its own tree: a
        // commit adds history rather than rewriting it.
        let parent_read = ok(
            &router,
            "GetCommit",
            &json!({ "repositoryName": repository, "commitId": parent }),
        )
        .await;
        assert_eq!(
            string_at(&parent_read, "/commit/commitId"),
            parent,
            "case {label:?}: the parent commit is still readable"
        );
    }
}

#[tokio::test]
async fn get_commit_refuses_an_id_the_repository_does_not_hold() {
    let router = emulator();
    let repository = "get-commit-repo";
    let other = "get-commit-other";
    let known = seed_repository(&router, repository, "main").await;
    let other_tip = seed_repository(&router, other, "main").await;
    // A commit only the other repository holds. It has to differ in content from
    // every commit of `repository`: a commit id is the object name of the commit,
    // so two byte-identical commits in two repositories ARE the same id.
    let elsewhere = string_at(
        &ok(
            &router,
            "CreateCommit",
            &json!({
                "repositoryName": other,
                "branchName": "main",
                "parentCommitId": other_tip,
                "commitMessage": "only in the other repository",
                "authorName": "Ada Lovelace",
                "email": "ada@example.com",
                "putFiles": [{ "filePath": "only-here.txt", "fileContent": base64(b"only here") }],
            }),
        )
        .await,
        "/commitId",
    );

    // Rows: well-formed object names no commit of this repository carries — the one
    // the recorded run used, digests of other seeds, all zeroes and all f's, and the
    // id of a commit that exists in a DIFFERENT repository.
    let unknown: Vec<(String, String)> = vec![
        (
            "a well-formed id no repository holds".to_string(),
            "0d0c1f049b4e446ac20e9b10824eb5151f3ab290".to_string(),
        ),
        (
            "another well-formed id".to_string(),
            absent_object_id("no such commit"),
        ),
        ("an id of all zeroes".to_string(), "0".repeat(40)),
        ("an id of all f's".to_string(), "f".repeat(40)),
        (
            "an id in upper case".to_string(),
            absent_object_id("upper").to_uppercase(),
        ),
        (
            "the id of a commit in another repository".to_string(),
            elsewhere.clone(),
        ),
    ];
    for (label, commit_id) in unknown {
        let code = error(
            &router,
            "GetCommit",
            &json!({ "repositoryName": repository, "commitId": commit_id }),
            400,
        )
        .await;
        assert_eq!(
            code, "CommitIdDoesNotExistException",
            "case {label:?}: expected CommitIdDoesNotExistException for {commit_id:?}, \
             got {code:?}"
        );
    }

    // A value that is not a git object name at all is a different refusal.
    let malformed: Vec<(&str, String)> = vec![
        ("prose", "not-a-commit-id".to_string()),
        ("too short", "abc123".to_string()),
        ("39 hex characters", "a".repeat(39)),
        ("41 hex characters", "a".repeat(41)),
        ("40 characters, not all hex", format!("{}z", "a".repeat(39))),
    ];
    for (label, commit_id) in malformed {
        let code = error(
            &router,
            "GetCommit",
            &json!({ "repositoryName": repository, "commitId": commit_id }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidCommitIdException",
            "case {label:?}: expected InvalidCommitIdException for {commit_id:?}, got {code:?}"
        );
    }

    // A repository that does not exist names the repository.
    let code = error(
        &router,
        "GetCommit",
        &json!({ "repositoryName": "ghost-repo", "commitId": known }),
        400,
    )
    .await;
    assert_eq!(
        code, "RepositoryDoesNotExistException",
        "case \"a repository that does not exist\": expected RepositoryDoesNotExistException, \
         got {code:?}"
    );

    // The commit the repository does hold is still answered, and reading it twice
    // answers the same thing.
    let first = ok(
        &router,
        "GetCommit",
        &json!({ "repositoryName": repository, "commitId": known }),
    )
    .await;
    let second = ok(
        &router,
        "GetCommit",
        &json!({ "repositoryName": repository, "commitId": known }),
    )
    .await;
    assert_eq!(
        first, second,
        "case \"the same commit read twice\": a read must not change what a later caller sees"
    );
}

// ---------------------------------------------------------------------------
// 5. CreateCommit's entry-level refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_commit_whose_entries_cannot_be_applied_is_refused_with_the_modeled_error() {
    let router = emulator();
    let repository = "commit-refusal-repo";
    let parent = seed_repository(&router, repository, "main").await;

    // Rows: one per way a set of entries cannot be applied to the tree.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "both file content and a source file for one path",
            json!({
                "putFiles": [{
                    "filePath": "a.txt",
                    "fileContent": base64(b"a"),
                    "sourceFile": { "filePath": "README.md" },
                }],
            }),
            "FileContentAndSourceFileSpecifiedException",
        ),
        (
            "neither file content nor a source file",
            json!({ "putFiles": [{ "filePath": "a.txt" }] }),
            "SourceFileOrContentRequiredException",
        ),
        (
            "a source file the tree does not hold",
            json!({
                "putFiles": [{
                    "filePath": "a.txt",
                    "sourceFile": { "filePath": "nowhere.txt" },
                }],
            }),
            "FileDoesNotExistException",
        ),
        (
            "a source file that exists only as another entry of this request",
            json!({
                "putFiles": [
                    { "filePath": "fresh.txt", "fileContent": base64(b"fresh") },
                    { "filePath": "copy.txt", "sourceFile": { "filePath": "fresh.txt" } },
                ],
            }),
            "FileDoesNotExistException",
        ),
        (
            "a delete of a file the tree does not hold",
            json!({ "deleteFiles": [{ "filePath": "nowhere.txt" }] }),
            "FileDoesNotExistException",
        ),
        (
            "a mode change for a file the tree does not hold",
            json!({
                "setFileModes": [{ "filePath": "nowhere.txt", "fileMode": "EXECUTABLE" }],
            }),
            "FileDoesNotExistException",
        ),
        (
            "two changes to the same path",
            json!({
                "putFiles": [{ "filePath": "a.txt", "fileContent": base64(b"a") }],
                "deleteFiles": [{ "filePath": "a.txt" }],
            }),
            "SamePathRequestException",
        ),
        (
            "the same delete twice",
            json!({
                "deleteFiles": [{ "filePath": "README.md" }, { "filePath": "README.md" }],
            }),
            "SamePathRequestException",
        ),
        (
            "one path used as both a file and a folder",
            json!({
                "putFiles": [
                    { "filePath": "a.txt", "fileContent": base64(b"a") },
                    { "filePath": "a.txt/b.txt", "fileContent": base64(b"b") },
                ],
            }),
            "PutFileEntryConflictException",
        ),
        (
            "a put whose path names a folder the tree already holds",
            json!({ "putFiles": [{ "filePath": "src", "fileContent": base64(b"x") }] }),
            "FileNameConflictsWithDirectoryNameException",
        ),
        (
            "a put under a path the tree holds as a file",
            json!({
                "putFiles": [{ "filePath": "README.md/inner", "fileContent": base64(b"x") }],
            }),
            "DirectoryNameConflictsWithFileNameException",
        ),
        (
            "a put whose path is empty",
            json!({ "putFiles": [{ "filePath": "/", "fileContent": base64(b"x") }] }),
            "PathRequiredException",
        ),
        (
            "a put whose path walks out of the tree",
            json!({ "putFiles": [{ "filePath": "../a.txt", "fileContent": base64(b"x") }] }),
            "InvalidPathException",
        ),
        (
            "a commit that changes nothing",
            json!({
                "putFiles": [{ "filePath": "README.md", "fileContent": base64(b"# demo\n") }],
            }),
            "NoChangeException",
        ),
        (
            "a move whose source the same commit also deletes",
            json!({
                "putFiles": [{
                    "filePath": "docs/README.md",
                    "sourceFile": { "filePath": "README.md", "isMove": true },
                }],
                "deleteFiles": [{ "filePath": "README.md" }],
            }),
            "SamePathRequestException",
        ),
    ];

    for (label, change, expected) in cases {
        let mut request = json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": parent,
        });
        for (member, value) in change
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: the change is an object"))
        {
            request[member] = value.clone();
        }
        let code = error(&router, "CreateCommit", &request, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code:?}"
        );
        // Nothing was committed: the branch still points at the seeded tip.
        let branch = ok(
            &router,
            "GetBranch",
            &json!({ "repositoryName": repository, "branchName": "main" }),
        )
        .await;
        assert_eq!(
            string_at(&branch, "/branch/commitId"),
            parent,
            "case {label:?}: a refused commit leaves the branch tip where it was"
        );
    }

    // The parent commit id is part of the contract too: it is required on a branch
    // that has a tip, must be that tip, and must be a commit the repository holds.
    let parent_cases: Vec<(&str, Value, &str)> = vec![
        (
            "no parent commit id on a branch with a tip",
            json!({}),
            "ParentCommitIdRequiredException",
        ),
        (
            "a parent commit id that is not an object name",
            json!({ "parentCommitId": "not-a-commit-id" }),
            "InvalidParentCommitIdException",
        ),
        (
            "a parent commit id no commit of this repository carries",
            json!({ "parentCommitId": absent_object_id("another parent") }),
            "ParentCommitDoesNotExistException",
        ),
    ];
    for (label, parent_member, expected) in parent_cases {
        let mut request = json!({
            "repositoryName": repository,
            "branchName": "main",
            "putFiles": [{ "filePath": "a.txt", "fileContent": base64(b"a") }],
        });
        for (member, value) in parent_member
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: the member set is an object"))
        {
            request[member] = value.clone();
        }
        let code = error(&router, "CreateCommit", &request, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code:?}"
        );
    }

    // An outdated parent is told apart from one that does not exist: a second
    // commit moves the tip, and the first id then names a commit that is no longer
    // the tip.
    let moved = ok(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": parent,
            "putFiles": [{ "filePath": "a.txt", "fileContent": base64(b"a") }],
        }),
    )
    .await;
    assert_ne!(
        string_at(&moved, "/commitId"),
        parent,
        "case \"a second commit\": the tip moves to a new commit"
    );
    let code = error(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": parent,
            "putFiles": [{ "filePath": "b.txt", "fileContent": base64(b"b") }],
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "ParentCommitIdOutdatedException",
        "case \"a parent commit id that is no longer the tip\": expected \
         ParentCommitIdOutdatedException, got {code:?}"
    );
}

#[tokio::test]
async fn keeping_empty_folders_keeps_the_whole_structure_a_deletion_empties() {
    // Rows: the same deletion with and without `keepEmptyFolders`. Kept, the
    // placeholder goes in the DEEPEST emptied folder, which is what keeps every
    // folder above it non-empty — so the structure survives with one placeholder.
    let cases: Vec<(&str, bool, Vec<&str>, Vec<&str>)> = vec![
        (
            "keepEmptyFolders true",
            true,
            vec!["deep/one/two/.gitkeep"],
            vec!["deep/.gitkeep", "deep/one/.gitkeep"],
        ),
        (
            "keepEmptyFolders absent",
            false,
            vec![],
            vec![
                "deep/.gitkeep",
                "deep/one/.gitkeep",
                "deep/one/two/.gitkeep",
            ],
        ),
    ];
    for (label, keep, expected, absent) in cases {
        // One repository per row, so each starts from the same tree.
        let router = emulator();
        let repository = "keep-folders-repo";
        let seeded = seed_repository(&router, repository, "main").await;

        // A file several folders deep, so deleting it empties a CHAIN of folders.
        let written = ok(
            &router,
            "CreateCommit",
            &json!({
                "repositoryName": repository,
                "branchName": "main",
                "parentCommitId": seeded,
                "putFiles": [{
                    "filePath": "deep/one/two/file.txt",
                    "fileContent": base64(b"deep"),
                }],
            }),
        )
        .await;
        let tip = string_at(&written, "/commitId");

        let mut request = json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": tip,
            "deleteFiles": [{ "filePath": "deep/one/two/file.txt" }],
        });
        if keep {
            request["keepEmptyFolders"] = json!(true);
        }
        let committed = ok(&router, "CreateCommit", &request).await;
        let commit_id = string_at(&committed, "/commitId");
        let added = paths_of(&committed, "filesAdded", label);
        assert_eq!(
            added,
            expected.iter().map(|path| (*path).to_string()).collect(),
            "case {label:?}: filesAdded names the placeholders this commit wrote"
        );
        for path in &expected {
            let file = ok(
                &router,
                "GetFile",
                &json!({
                    "repositoryName": repository,
                    "commitSpecifier": commit_id,
                    "filePath": path,
                }),
            )
            .await;
            assert_eq!(
                from_base64(&string_at(&file, "/fileContent")),
                Vec::<u8>::new(),
                "case {label:?}: the placeholder at {path} is an empty file"
            );
        }
        for path in &absent {
            let code = error(
                &router,
                "GetFile",
                &json!({
                    "repositoryName": repository,
                    "commitSpecifier": commit_id,
                    "filePath": path,
                }),
                400,
            )
            .await;
            assert_eq!(
                code, "FileDoesNotExistException",
                "case {label:?}: no placeholder belongs at {path}, got {code:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 7. The bounds CreateCommit publishes on what one request may carry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_commit_is_bounded_on_the_axes_a_caller_drives() {
    use service_com_amazonaws_codecommit_codecommit_20150413::state::{
        MAX_COMBINED_CONTENT_BYTES, MAX_FILE_ENTRIES_PER_COMMIT,
    };

    let router = emulator();
    let repository = "commit-bounds-repo";
    let parent = seed_repository(&router, repository, "main").await;

    // The entry count: at the bound the commit is served, one past it refused.
    let entries = |count: usize| -> Value {
        (0..count)
            .map(|index| {
                json!({
                    "filePath": format!("many/file-{index}.txt"),
                    "fileContent": base64(format!("{index}").as_bytes()),
                })
            })
            .collect()
    };
    let code = error(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": parent,
            "putFiles": entries(MAX_FILE_ENTRIES_PER_COMMIT + 1),
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "MaximumFileEntriesExceededException",
        "case \"one entry past the per-commit maximum\": expected \
         MaximumFileEntriesExceededException, got {code:?}"
    );
    let served = ok(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": parent,
            "putFiles": entries(MAX_FILE_ENTRIES_PER_COMMIT),
        }),
    )
    .await;
    let at_bound = string_at(&served, "/commitId");
    assert!(
        is_object_id(&at_bound),
        "case \"entries at the per-commit maximum\": the commit is served, got {served}"
    );

    // The combined content size, which counts every byte the commit writes: a put
    // of half the bound is served, two of them are not, and content pulled in
    // through a `sourceFile` copy counts exactly as inline content does.
    // Each of these is just over half the combined bound, so one is served and any
    // two of them break it.
    let half = vec![b'x'; MAX_COMBINED_CONTENT_BYTES / 2 + 1];
    let written = ok(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": at_bound,
            "putFiles": [{ "filePath": "big/one.bin", "fileContent": base64(&half) }],
        }),
    )
    .await;
    let big_tip = string_at(&written, "/commitId");
    assert_eq!(
        from_base64(&string_at(
            &ok(
                &router,
                "GetFile",
                &json!({
                    "repositoryName": repository,
                    "commitSpecifier": big_tip,
                    "filePath": "big/one.bin",
                }),
            )
            .await,
            "/fileContent"
        ))
        .len(),
        half.len(),
        "case \"a put of half the combined bound\": the content is stored whole"
    );

    let code = error(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": big_tip,
            "putFiles": [
                { "filePath": "big/two.bin", "fileContent": base64(&half) },
                { "filePath": "big/three.bin", "fileContent": base64(&half) },
            ],
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "FileContentSizeLimitExceededException",
        "case \"two inline puts past the combined bound\": expected \
         FileContentSizeLimitExceededException, got {code:?}"
    );
    let code = error(
        &router,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "parentCommitId": big_tip,
            "putFiles": [
                { "filePath": "big/two.bin", "fileContent": base64(&half) },
                {
                    "filePath": "big/copy.bin",
                    "sourceFile": { "filePath": "big/one.bin" },
                },
            ],
        }),
        400,
    )
    .await;
    assert_eq!(
        code, "FileContentSizeLimitExceededException",
        "case \"an inline put plus a copy past the combined bound\": expected \
         FileContentSizeLimitExceededException, got {code:?}"
    );

    // Every refusal above left the branch where the last served commit put it.
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": "main" }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        big_tip,
        "case \"after the bounds table\": a refused commit leaves the branch tip where it was"
    );
}
