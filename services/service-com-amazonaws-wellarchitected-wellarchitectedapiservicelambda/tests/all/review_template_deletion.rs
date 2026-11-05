// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteReviewTemplate`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. `DeleteReviewTemplate` resolves its `TemplateArn` against the store and DROPS that
//!    template with everything held inside it, answering an empty 200; afterwards every
//!    operation that resolves a `TemplateArn` reports the ARN as absent, and no other
//!    stored template is touched.
//! 2. A `TemplateArn` naming no stored template is a `404 ResourceNotFoundException`
//!    carrying the ARN as sent and the missing resource's type — which a delete of an
//!    already-deleted template under a `ClientRequestToken` this service has not spent is
//!    a case of, so the delete persists rather than being undone. (A delete repeated under
//!    the ORIGINAL token is the `@idempotencyToken` replay, pinned in
//!    `delete_idempotency`.)
//! 3. A delete frees what was keyed to the template: the `ClientRequestToken` that minted
//!    it no longer replays, so the same token creates a fresh template rather than
//!    answering the ARN of a template that is gone.
//! 4. A workload that a template was APPLIED to is not a child of that template — the
//!    workload copied the template's lenses and notes when it was created — so deleting
//!    the template leaves the workload exactly as it was.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{
    answered, created_arn, created_id, minimal_create, minimal_template, official_lens_arn, Answer,
    Client,
};

/// The AWS-official Well-Architected Framework lens, named by its alias; its ARN is the
/// other spelling the same lens answers to ("either the lens alias … or the lens ARN" —
/// `smithy_query(query="shape", shape="LensAlias")`), built by the harness so the
/// account and Region in it stay derived rather than literal.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens AWS publishes whose question set this emulator does not ship — a row that
/// varies the applied lens along "does the emulator hold its content" without changing
/// what a delete has to do.
const UNSHIPPED_LENS: &str = "serverless";

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A well-formed `TemplateArn` (the modelled
/// `^arn:aws…:wellarchitected:<region>:<account>:review-template/[a-f0-9]{32}$`) in
/// `region` carrying `template_id`.
fn template_arn_in(region: &str, template_id: &str) -> String {
    format!(
        "arn:aws:wellarchitected:{region}:{}:review-template/{template_id}",
        engine::emulator_account_id()
    )
}

/// Every assertion a deleted `TemplateArn` has to satisfy: the 404 envelope the service
/// words in ONE place for every operation that resolves a `TemplateArn`.
fn assert_template_absent(answer: &Answer, template_arn: &str, case: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404 for {template_arn}, got {answer:?}"
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
        "case {case:?}: expected the ARN/account not-found message, got {answer:?}"
    );
}

/// One row of rule 1: a template to create, what to write to it before the delete, and
/// the lens whose review the delete must take with it.
struct DeleteCase {
    case: &'static str,
    /// Overrides merged into [`minimal_template`].
    overrides: Value,
    /// The lens AS THE READS NAME IT, for the lens-review read after the delete.
    lens: String,
    /// Whether to write lens-review notes before deleting.
    write_notes: bool,
    /// The question to record an answer against before deleting, if any.
    answer_question: Option<&'static str>,
}

/// Rule 1. The rows differ along what the deleted template HOLDS — how many lenses it
/// applies, how a lens is named (alias or ARN), whether the emulator ships that lens's
/// question set, whether the template carries notes and tags, and whether a caller has
/// written lens-review notes or recorded an answer against it — because a delete drops
/// the template whatever it holds, and nothing it held may survive it.
#[tokio::test]
async fn deleting_a_review_template_drops_it_and_everything_it_holds() {
    // The ARN spelling of the framework lens, built from the harness's one official-lens
    // ARN builder.
    let framework_lens_arn = official_lens_arn("us-east-1", FRAMEWORK_LENS);
    let cases: Vec<DeleteCase> = vec![
        DeleteCase {
            case: "minimal",
            overrides: json!({}),
            lens: FRAMEWORK_LENS.to_string(),
            write_notes: false,
            answer_question: None,
        },
        DeleteCase {
            case: "notes-and-tags",
            overrides: json!({
                "Notes": "notes carried to a workload the template is applied to",
                "Tags": { "Env": "test", "Team": "core" },
            }),
            lens: FRAMEWORK_LENS.to_string(),
            write_notes: false,
            answer_question: None,
        },
        DeleteCase {
            case: "no-lenses",
            overrides: json!({ "Lenses": [] }),
            lens: FRAMEWORK_LENS.to_string(),
            write_notes: false,
            answer_question: None,
        },
        DeleteCase {
            case: "several-lenses",
            overrides: json!({
                "Lenses": [FRAMEWORK_LENS, UNSHIPPED_LENS, "softwareasaservice"],
            }),
            lens: UNSHIPPED_LENS.to_string(),
            write_notes: true,
            answer_question: None,
        },
        DeleteCase {
            case: "lens-named-by-arn",
            overrides: json!({ "Lenses": [framework_lens_arn] }),
            lens: framework_lens_arn.clone(),
            write_notes: true,
            answer_question: None,
        },
        DeleteCase {
            case: "with-lens-review-notes",
            overrides: json!({}),
            lens: FRAMEWORK_LENS.to_string(),
            write_notes: true,
            answer_question: None,
        },
        DeleteCase {
            case: "with-a-recorded-answer",
            overrides: json!({}),
            lens: FRAMEWORK_LENS.to_string(),
            write_notes: false,
            answer_question: Some("organization-priorities"),
        },
        DeleteCase {
            case: "with-notes-and-an-answer",
            overrides: json!({}),
            lens: FRAMEWORK_LENS.to_string(),
            write_notes: true,
            answer_question: Some("detect-investigate-events"),
        },
    ];

    // One client, so every row shares a store: the bystander template below is created
    // once and must survive every delete.
    let client = Client::new();
    let bystander = created_arn(
        &client
            .create_review_template(minimal_template("bystander-template", "tok-bystander"))
            .await,
        "bystander",
    );

    for case in cases {
        let name = case.case;
        let mut request = minimal_template(&format!("template-{name}"), &format!("tok-{name}"));
        for (member_name, value) in case.overrides.as_object().expect("overrides are an object") {
            request[member_name] = value.clone();
        }
        let arn = created_arn(&client.create_review_template(request).await, name);

        if case.write_notes {
            let written = client
                .update_review_template_lens_review(
                    &arn,
                    &case.lens,
                    json!({ "LensNotes": format!("notes written for {name}") }),
                )
                .await;
            assert_eq!(
                written.status, 200,
                "case {name:?}: the lens-review notes must be written before the delete, got {written:?}"
            );
        }
        if let Some(question_id) = case.answer_question {
            client.record_template_answer(
                &arn,
                FRAMEWORK_LENS,
                question_id,
                answered(&[("bp-1", "SELECTED")], true, None),
            );
        }

        // The template resolves before the delete, so what the rows assert afterwards is
        // the delete and not a template that was never stored.
        let before = client.get_review_template(&arn).await;
        assert_eq!(
            before.status, 200,
            "case {name:?}: the template must resolve before the delete, got {before:?}"
        );

        let deleted = client
            .delete_review_template(&arn, &format!("tok-delete-{name}"))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: expected DeleteReviewTemplate 200, got {deleted:?}"
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {name:?}: DeleteReviewTemplate models no output members, so the body is \
             empty, got {deleted:?}"
        );

        // Every operation that resolves a `TemplateArn` now reports it absent — the
        // template itself, and the lens review and answers it held.
        assert_template_absent(
            &client.get_review_template(&arn).await,
            &arn,
            &format!("{name}/GetReviewTemplate"),
        );
        assert_template_absent(
            &client
                .get_review_template_lens_review(&arn, &case.lens)
                .await,
            &arn,
            &format!("{name}/GetReviewTemplateLensReview"),
        );
        assert_template_absent(
            &client
                .update_review_template_lens_review(&arn, &case.lens, json!({ "LensNotes": "x" }))
                .await,
            &arn,
            &format!("{name}/UpdateReviewTemplateLensReview"),
        );

        // Rule 1's last clause: no OTHER template is touched.
        let survivor = client.get_review_template(&bystander).await;
        assert_eq!(
            survivor.status, 200,
            "case {name:?}: deleting one template must leave every other one stored, got {survivor:?}"
        );
    }

    // And the bystander is still deletable at the end, so the rows above left the store
    // in working order rather than wedged.
    let last = client
        .delete_review_template(&bystander, "tok-delete-bystander")
        .await;
    assert_eq!(
        last.status, 200,
        "case \"bystander\": the surviving template must still be deletable, got {last:?}"
    );
}

/// Rule 2. The rows differ along WHY the ARN resolves to nothing — never minted, minted
/// in another Region, deleted a moment ago, deleted twice — because a `TemplateArn` is
/// resolved against the store rather than parsed. Every row carries a `ClientRequestToken`
/// this service has not spent, so each is a NEW request rather than a retry.
#[tokio::test]
async fn deleting_a_template_arn_that_names_no_template_is_not_found() {
    let client = Client::new();
    let present = created_arn(
        &client
            .create_review_template(minimal_template("present-template", "tok-present"))
            .await,
        "present",
    );
    // A template created and then deleted: its ARN is well formed and was real.
    let deleted = created_arn(
        &client
            .create_review_template(minimal_template("deleted-template", "tok-deleted"))
            .await,
        "deleted",
    );
    assert_eq!(
        client
            .delete_review_template(&deleted, "tok-delete-deleted")
            .await
            .status,
        200,
        "the row's template must be deletable once"
    );
    // The present template's id in ANOTHER Region: the same resource part, a different
    // ARN, so a suffix match would wrongly resolve it. The id is taken from the minted
    // ARN rather than from a literal, so the row does not depend on the Region spelling.
    let present_id = present
        .rsplit('/')
        .next()
        .expect("a minted TemplateArn ends in review-template/<id>")
        .to_string();
    let other_region = template_arn_in("eu-central-1", &present_id);

    let cases: Vec<(&str, String)> = vec![
        (
            "never-minted",
            template_arn_in("us-west-2", "0123456789abcdef0123456789abcdef"),
        ),
        (
            "never-minted-all-f",
            template_arn_in("us-west-2", "ffffffffffffffffffffffffffffffff"),
        ),
        ("another-region", other_region),
        ("already-deleted", deleted.clone()),
    ];

    for (case, template_arn) in cases {
        assert_ne!(
            template_arn, present,
            "case {case:?}: the row must not name the template that exists"
        );
        let answer = client
            .delete_review_template(&template_arn, &format!("tok-missing-{case}"))
            .await;
        assert_template_absent(&answer, &template_arn, case);
    }

    // Deleting the same template again under a token the service has not spent is still
    // the same refusal: the delete persisted rather than being undone.
    let again = client
        .delete_review_template(&deleted, "tok-delete-deleted-again")
        .await;
    assert_template_absent(&again, &deleted, "repeat-delete-under-a-fresh-token");

    // The template that DOES exist still resolves, so the rows above failed on resolution
    // and not on a broken route.
    let fetched = client.get_review_template(&present).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored template must still resolve, got {fetched:?}"
    );
}

/// Rule 3. The rows differ along how many templates one `ClientRequestToken` has created
/// and deleted in turn, because the token resolves to a LIVE template: once that template
/// is gone the token names nothing to replay, however often the cycle repeats.
#[tokio::test]
async fn a_delete_frees_the_client_request_token_that_minted_the_template() {
    let client = Client::new();
    const TOKEN: &str = "tok-recreated";
    let mut minted: Vec<String> = Vec::new();

    for round in 1..=3 {
        let case = format!("round-{round}");
        let created = client
            .create_review_template(minimal_template("recreated-template", TOKEN))
            .await;
        let arn = created_arn(&created, &case);
        assert!(
            !minted.contains(&arn),
            "case {case:?}: the token must mint a FRESH template once the last one was \
             deleted, got {arn:?} again"
        );
        minted.push(arn.clone());

        // While the template is live the same token replays rather than creating a second.
        let replay = client
            .create_review_template(minimal_template("recreated-template", TOKEN))
            .await;
        assert_eq!(
            created_arn(&replay, &case),
            arn,
            "case {case:?}: the live token must replay the original template"
        );

        let deleted = client
            .delete_review_template(&arn, &format!("tok-delete-{case}"))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteReviewTemplate 200, got {deleted:?}"
        );
    }

    // Every ARN the token ever minted is gone.
    for (index, arn) in minted.iter().enumerate() {
        assert_template_absent(
            &client.get_review_template(arn).await,
            arn,
            &format!("minted-{index}"),
        );
    }
}

/// Rule 4. The rows differ along what the workload took from the template — its lenses
/// alone, its lenses and its notes, or lenses it already carried itself — because a
/// workload copies what it applies at create time and is no child of the template.
#[tokio::test]
async fn deleting_a_template_leaves_a_workload_it_was_applied_to() {
    // (case, the template's request overrides, the workload's request overrides)
    let cases: Vec<(&str, Value, Value)> = vec![
        ("lenses-only", json!({}), json!({})),
        (
            "notes-from-the-template",
            json!({ "Notes": "notes the workload inherits" }),
            json!({ "Notes": null }),
        ),
        (
            "workload-brings-its-own-notes",
            json!({ "Notes": "notes the workload does NOT inherit" }),
            json!({ "Notes": "the workload's own notes" }),
        ),
        (
            "template-adds-a-lens-the-workload-lacks",
            json!({ "Lenses": [FRAMEWORK_LENS, UNSHIPPED_LENS] }),
            json!({ "Lenses": [FRAMEWORK_LENS] }),
        ),
    ];

    let client = Client::new();
    for (case, template_overrides, workload_overrides) in cases {
        let mut template_request =
            minimal_template(&format!("applied-template-{case}"), &format!("tok-t-{case}"));
        for (member_name, value) in template_overrides
            .as_object()
            .expect("overrides are an object")
        {
            template_request[member_name] = value.clone();
        }
        let arn = created_arn(&client.create_review_template(template_request).await, case);

        let mut workload_request =
            minimal_create(&format!("applied-workload-{case}"), &format!("tok-w-{case}"));
        workload_request["ReviewTemplateArns"] = json!([arn]);
        for (member_name, value) in workload_overrides
            .as_object()
            .expect("overrides are an object")
        {
            if value.is_null() {
                workload_request
                    .as_object_mut()
                    .expect("the request is an object")
                    .remove(member_name);
            } else {
                workload_request[member_name] = value.clone();
            }
        }
        let workload_id = created_id(&client.create(workload_request).await, case);

        let before = client.get(&workload_id).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the workload must resolve before the delete, got {before:?}"
        );

        let deleted = client
            .delete_review_template(&arn, &format!("tok-delete-{case}"))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteReviewTemplate 200, got {deleted:?}"
        );

        let after = client.get(&workload_id).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: the workload must outlive the template it applied, got {after:?}"
        );
        assert_eq!(
            after.body, before.body,
            "case {case:?}: deleting the template must not change the workload it was \
             applied to"
        );
    }
}
