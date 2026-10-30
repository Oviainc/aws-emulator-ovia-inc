// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `ResourceNotFoundException` a review-template ARN that
//! resolves to nothing is refused with.
//!
//! The rule pinned here, over a TABLE of operations and ARNs rather than one recorded
//! request: every operation that resolves a `TemplateArn` against the store answers an ARN
//! naming no stored template with a modelled `404 ResourceNotFoundException`
//! (`smithy_query(query="operation", operation="GetReviewTemplate")` lists it with
//! `httpError 404`) whose `Message` names the resource class by the service's own display
//! noun for it — the capitalised `Review Template` — with the ARN AS SENT and the account
//! the lookup ran in. One wording, from whichever operation did the lookup.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{
    created_arn, member, minimal_create, minimal_template, Answer, Client,
};

/// A well-formed `TemplateArn` (the modelled
/// `^arn:aws…:wellarchitected:<region>:<account>:review-template/[a-f0-9]{32}$`) that no
/// stored template carries.
fn absent_template_arn(template_id: &str) -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:review-template/{template_id}",
        engine::emulator_account_id()
    )
}

/// The one not-found envelope every row must answer with.
fn assert_template_not_found(case: &str, answer: &Answer, template_arn: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        "REVIEW_TEMPLATE",
        "case {case:?}: expected ResourceType REVIEW_TEMPLATE, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        template_arn,
        "case {case:?}: ResourceId must be the ARN as sent, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!(
            "[ResourceNotFound] No Review Template with ARN {template_arn} was found in \
             account {}.",
            engine::emulator_account_id()
        ),
        "case {case:?}: the message must name the resource class as the service words it, \
         got {answer:?}"
    );
}

/// Every operation that resolves a `TemplateArn` words the refusal the same way. The rows
/// differ along the operation that does the lookup and along the ARN: ids no mint produces
/// in this account, an id one character off a STORED template's, and a stored template's
/// ARN in another Region.
#[tokio::test]
async fn every_operation_words_a_missing_review_template_the_same_way() {
    let client = Client::new();

    // A template that DOES exist, so a row that is refused is refused on resolution
    // rather than on an empty store.
    let present = created_arn(
        &client
            .create_review_template(minimal_template("present-template", "tok-present"))
            .await,
        "present",
    );
    let present_id = present
        .rsplit('/')
        .next()
        .expect("the ARN carries a template id")
        .to_string();

    let arns: Vec<(&str, String)> = vec![
        ("never-minted-zeros", absent_template_arn(&"0".repeat(32))),
        ("never-minted-f", absent_template_arn(&"f".repeat(32))),
        (
            "never-minted-mixed",
            absent_template_arn("0123456789abcdef0123456789abcdef"),
        ),
        (
            "one-character-off-a-stored-id",
            absent_template_arn(&format!(
                "{}{}",
                &present_id[..31],
                if present_id.ends_with('0') { '1' } else { '0' }
            )),
        ),
        (
            "stored-id-in-another-region",
            present.replace("us-west-2", "eu-west-1"),
        ),
    ];

    for (row, template_arn) in arns {
        assert_ne!(
            template_arn, present,
            "case {row:?}: the row must not name the template that exists"
        );

        // One row per operation that resolves the ARN, each crossing the HTTP boundary.
        let answers: Vec<(&str, Answer)> = vec![
            ("GetReviewTemplate", client.get_review_template(&template_arn).await),
            (
                "UpdateReviewTemplate",
                client
                    .update_review_template(&template_arn, json!({ "Description": "a revision" }))
                    .await,
            ),
            (
                "DeleteReviewTemplate",
                client
                    .delete_review_template(&template_arn, &format!("tok-del-{row}"))
                    .await,
            ),
            (
                "GetReviewTemplateLensReview",
                client
                    .get_review_template_lens_review(&template_arn, "wellarchitected")
                    .await,
            ),
            (
                "ListReviewTemplateAnswers",
                client
                    .list_review_template_answers(&template_arn, "wellarchitected", "")
                    .await,
            ),
            (
                "CreateTemplateShare",
                super::template_shares::create_share(
                    &client,
                    &template_arn,
                    "111122223333",
                    &format!("tok-share-{row}"),
                )
                .await,
            ),
            (
                "ListTemplateShares",
                super::template_shares::list_shares(&client, &template_arn, "").await,
            ),
            (
                "CreateWorkload",
                client
                    .create({
                        let mut request =
                            minimal_create(&format!("wl-{row}"), &format!("tok-wl-{row}"));
                        request["ReviewTemplateArns"] = json!([template_arn]);
                        request
                    })
                    .await,
            ),
        ];

        for (operation, answer) in answers {
            assert_template_not_found(&format!("{operation}/{row}"), &answer, &template_arn);
        }
    }

    // The template that DOES exist still resolves, so the rows above fail on resolution
    // and not on a broken route.
    let fetched = client.get_review_template(&present).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored template must still resolve, got {fetched:?}"
    );
    assert_eq!(
        fetched
            .body
            .get("ReviewTemplate")
            .and_then(|template| template.get("TemplateName"))
            .and_then(Value::as_str),
        Some("present-template"),
        "case \"present\": the stored template must read back, got {fetched:?}"
    );
}
