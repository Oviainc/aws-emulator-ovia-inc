// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the review template lifecycle: `CreateReviewTemplate`,
//! the `GetReviewTemplate` that reads it back, and the `CreateWorkload` that applies
//! one.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. `CreateReviewTemplate` mints a review template, persists every member the request
//!    carries, and answers its `TemplateArn`; a later `GetReviewTemplate` of that ARN
//!    reports those members unchanged plus the ones the SERVICE computes (`Owner`,
//!    `UpdatedAt`, `UpdateStatus`, `QuestionCounts`), derived from the template's own
//!    stored state.
//! 2. A `ClientRequestToken` makes the create idempotent: the same token with the same
//!    parameters returns the original template, the same token with different parameters
//!    is a `409 ConflictException`, and a fresh token creates a second template.
//! 3. A `TemplateArn` is RESOLVED against the store: one naming no stored template is a
//!    `404 ResourceNotFoundException` carrying the ARN as sent and the missing
//!    resource's type.
//! 4. A review template applied to a new workload contributes its lenses (and, when the
//!    request brings none of its own, its notes) to that workload, and is part of the
//!    request that workload's own `ClientRequestToken` identifies.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// How many questions the AWS Well-Architected Framework lens asks: the framework's
/// published question numbering — OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11,
/// SUS 1-6. Spelled out here so the test pins the number rather than reading it back
/// out of the code under test.
const FRAMEWORK_LENS_QUESTIONS: i64 = 11 + 11 + 13 + 5 + 11 + 6;

/// The `Question` buckets, from `smithy_query(query="shape", shape="Question")`.
const QUESTION_BUCKETS: [&str; 2] = ["UNANSWERED", "ANSWERED"];

/// A well-formed `TemplateArn` (the modelled
/// `^arn:aws…:wellarchitected:<region>:<account>:review-template/[a-f0-9]{32}$`) that no
/// stored template carries.
fn absent_template_arn(template_id: &str) -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:review-template/{template_id}",
        engine::emulator_account_id()
    )
}

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A minimal valid `CreateReviewTemplate` request; a caller overrides the members its
/// case varies.
fn minimal_template(name: &str, token: &str) -> Value {
    json!({
        "TemplateName": name,
        "Description": "a review template under test",
        "Lenses": ["wellarchitected"],
        "ClientRequestToken": token,
    })
}

/// The `TemplateArn` a successful `CreateReviewTemplate` answered with, checked against
/// the modelled `TemplateArn` pattern and its `@length(50, 250)`.
fn created_arn(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateReviewTemplate 200, got {answer:?}"
    );
    let arn = answer
        .body
        .get("TemplateArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let expected_prefix = format!(
        "arn:aws:wellarchitected:us-west-2:{}:review-template/",
        engine::emulator_account_id()
    );
    let id = arn.strip_prefix(&expected_prefix).unwrap_or_default();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: TemplateArn must be {expected_prefix}<32 hex>, got {arn:?}"
    );
    assert!(
        (50..=250).contains(&arn.len()),
        "case {case:?}: TemplateArn must satisfy @length(50, 250), got {} characters in {arn:?}",
        arn.len()
    );
    arn
}

/// The roll-up a template with `unanswered` outstanding questions reports: every
/// `Question` bucket present, `ANSWERED` at 0 because nothing can answer a review
/// template question yet.
fn expected_question_counts(unanswered: i64) -> Value {
    let mut counts = serde_json::Map::new();
    for bucket in QUESTION_BUCKETS {
        let count = if bucket == "UNANSWERED" { unanswered } else { 0 };
        counts.insert(bucket.to_string(), json!(count));
    }
    Value::Object(counts)
}

fn template_of(answer: &Answer) -> Value {
    answer
        .body
        .get("ReviewTemplate")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Rule 1. The rows differ along what the request carries — whether notes and tags are
/// present, how many lenses are applied, how a lens is named (alias or ARN), whether the
/// same lens is named twice, whether the emulator ships the lens's question set at all,
/// and the name/description lengths at the published bounds — because the stored
/// template is projected from its own state, not from one recorded request.
#[tokio::test]
async fn a_created_review_template_round_trips_through_get() {
    // (case, request overrides, questions the applied lenses ask)
    let cases: Vec<(&str, Value, i64)> = vec![
        ("minimal", json!({}), FRAMEWORK_LENS_QUESTIONS),
        (
            "notes-and-tags",
            json!({
                "Notes": "notes carried to a workload the template is applied to",
                "Tags": { "Env": "test", "Team": "core" },
            }),
            FRAMEWORK_LENS_QUESTIONS,
        ),
        ("no-lenses", json!({ "Lenses": [] }), 0),
        // "For Amazon Web Services official lenses, this is either the lens alias ... or
        // the lens ARN, such as arn:aws:wellarchitected:us-east-1::lens/serverless"
        // (`LensAlias` doc), so both names resolve to the same question set.
        (
            "lens-by-arn",
            json!({ "Lenses": ["arn:aws:wellarchitected:us-east-1::lens/wellarchitected"] }),
            FRAMEWORK_LENS_QUESTIONS,
        ),
        (
            "lens-named-twice",
            json!({
                "Lenses": [
                    "wellarchitected",
                    "arn:aws:wellarchitected:us-east-1::lens/wellarchitected"
                ]
            }),
            FRAMEWORK_LENS_QUESTIONS,
        ),
        // A lens whose definition this emulator does not ship asks no questions here, so
        // it adds nothing to the roll-up.
        ("unshipped-lens-only", json!({ "Lenses": ["serverless"] }), 0),
        (
            "framework-and-unshipped-lenses",
            json!({ "Lenses": ["wellarchitected", "serverless", "softwareasaservice"] }),
            FRAMEWORK_LENS_QUESTIONS,
        ),
        // `TemplateName` is @length(3, 100) and `Description` @length(3, 250): each at
        // its published minimum, then at its published maximum.
        (
            "name-and-description-at-the-minimum-length",
            json!({ "TemplateName": "abc", "Description": "xyz" }),
            FRAMEWORK_LENS_QUESTIONS,
        ),
        (
            "name-and-description-at-the-maximum-length",
            json!({ "TemplateName": "n".repeat(100), "Description": "d".repeat(250) }),
            FRAMEWORK_LENS_QUESTIONS,
        ),
        (
            "notes-at-the-maximum-length",
            json!({ "Notes": "o".repeat(2084) }),
            FRAMEWORK_LENS_QUESTIONS,
        ),
    ];

    // One client, so every template shares a store and the ARNs must stay distinct.
    let client = Client::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for (case, overrides, unanswered) in cases {
        let mut request = minimal_template(&format!("template-{case}"), &format!("tok-{case}"));
        for (name, value) in overrides.as_object().expect("overrides are an object") {
            request[name] = value.clone();
        }

        let created = client.create_review_template(request.clone()).await;
        let arn = created_arn(&created, case);
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: every created template must get its own ARN, but {arn:?} repeated"
        );

        let fetched = client.get_review_template(&arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetReviewTemplate must resolve the created ARN, got {fetched:?}"
        );
        let template = template_of(&fetched);

        // The members the REQUEST set come back exactly as sent.
        for name in ["TemplateName", "Description", "Lenses"] {
            assert_eq!(
                template.get(name),
                request.get(name),
                "case {case:?}: {name} must round-trip as sent, got {template:?}"
            );
        }
        assert_eq!(
            template.get("Notes").cloned().unwrap_or(Value::Null),
            request.get("Notes").cloned().unwrap_or(Value::Null),
            "case {case:?}: Notes must round-trip as sent, got {template:?}"
        );
        let expected_tags = request.get("Tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            template.get("Tags"),
            Some(&expected_tags),
            "case {case:?}: expected Tags {expected_tags:?}, got {template:?}"
        );

        // The members the SERVICE computes.
        assert_eq!(
            template.get("TemplateArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the template must report its own ARN, got {template:?}"
        );
        assert_eq!(
            template.get("Owner").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: the creating account owns the template, got {template:?}"
        );
        assert_eq!(
            template.get("UpdateStatus").and_then(Value::as_str),
            Some("CURRENT"),
            "case {case:?}: a template whose lenses are current reports CURRENT, got {template:?}"
        );
        let expected_counts = expected_question_counts(unanswered);
        assert_eq!(
            template.get("QuestionCounts"),
            Some(&expected_counts),
            "case {case:?}: expected QuestionCounts {expected_counts:?}, got {template:?}"
        );
        assert!(
            template
                .get("UpdatedAt")
                .and_then(Value::as_f64)
                .is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: UpdatedAt must be the epoch-seconds timestamp of the create, got {template:?}"
        );
        // Only a template reached through a share invitation carries one, and template
        // shares are not implemented.
        assert_eq!(
            template.get("ShareInvitationId").cloned(),
            None,
            "case {case:?}: an unshared template carries no ShareInvitationId, got {template:?}"
        );

        // A read must not change what a later caller sees.
        let reread = template_of(&client.get_review_template(&arn).await);
        assert_eq!(
            reread, template,
            "case {case:?}: a second GetReviewTemplate must report the same template"
        );
    }
}

/// A review template carries at most `state::MAX_LENSES_PER_REVIEW_TEMPLATE` lenses — a
/// quota this emulator sets, because `CreateReviewTemplateInput$Lenses` publishes no
/// `@length` of its own and the number is the one the service publishes for a
/// review-template lens list elsewhere (`ReviewTemplateLensAliases`, `@length(1, 10)`).
/// Being a quota and not a modelled constraint, it is answered with the modelled
/// `ServiceQuotaExceededException`. Checked at the ceiling, which is accepted, and one
/// past it, which is refused.
#[tokio::test]
async fn the_lens_ceiling_bounds_a_stored_review_template() {
    const MAX_LENSES: usize = state::MAX_LENSES_PER_REVIEW_TEMPLATE;

    let client = Client::new();
    // The rows need MAX_LENSES + 1 lenses that all resolve, so the quota is what the
    // over-the-ceiling row meets rather than lens resolution.
    assert!(
        state::AWS_OFFICIAL_LENS_ALIASES.len() > MAX_LENSES,
        "the AWS-official lens catalog must carry more than {MAX_LENSES} aliases to test the ceiling, got {}",
        state::AWS_OFFICIAL_LENS_ALIASES.len()
    );
    for (case, count) in [
        ("one-lens", 1usize),
        ("at-the-ceiling", MAX_LENSES),
        ("one-past-the-ceiling", MAX_LENSES + 1),
    ] {
        // Distinct aliases of lenses that exist, so the row varies the COUNT and nothing
        // else.
        let lenses: Vec<String> = state::AWS_OFFICIAL_LENS_ALIASES
            .iter()
            .take(count)
            .map(|alias| (*alias).to_string())
            .collect();
        let mut request = minimal_template(&format!("lenses-{case}"), &format!("tok-lenses-{case}"));
        request["Lenses"] = json!(lenses);

        let answer = client.create_review_template(request).await;
        if count <= MAX_LENSES {
            let arn = created_arn(&answer, case);
            let template = template_of(&client.get_review_template(&arn).await);
            assert_eq!(
                template.get("Lenses").and_then(Value::as_array).map(Vec::len),
                Some(count),
                "case {case:?}: a template at or under the ceiling must store all {count} lenses, got {template:?}"
            );
        } else {
            assert_eq!(
                answer.status, 402,
                "case {case:?}: more than {MAX_LENSES} lenses must meet the quota, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ServiceQuotaExceededException",
                "case {case:?}: expected ServiceQuotaExceededException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "QuotaCode"),
                "LensesPerReviewTemplate",
                "case {case:?}: the refusal must name the quota that was met, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                "REVIEW_TEMPLATE",
                "case {case:?}: expected ResourceType REVIEW_TEMPLATE, got {answer:?}"
            );
        }
    }
}

/// Rule 2. The rows differ along WHICH member the replayed token disagrees on, because
/// the token identifies the request by its whole parameter set, not by one member.
#[tokio::test]
async fn the_client_request_token_makes_the_create_idempotent() {
    let client = Client::new();
    let original = minimal_template("idempotent-template", "tok-idempotent");
    let first = client.create_review_template(original.clone()).await;
    let arn = created_arn(&first, "first-create");

    // The same token with the same parameters returns the original template.
    for attempt in 1..=2 {
        let replay = client.create_review_template(original.clone()).await;
        assert_eq!(
            replay.body.get("TemplateArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case \"replay {attempt}\": a replayed token must return the original ARN, got {replay:?}"
        );
    }

    // The same token with DIFFERENT parameters cannot be the same request.
    let divergences: Vec<(&str, Value)> = vec![
        ("TemplateName", json!("a-different-name")),
        ("Description", json!("a different description")),
        ("Lenses", json!(["serverless"])),
        ("Notes", json!("notes the original request did not carry")),
        ("Tags", json!({ "Env": "prod" })),
    ];
    for (member_name, value) in divergences {
        let mut request = original.clone();
        request[member_name] = value.clone();
        let answer = client.create_review_template(request).await;
        assert_eq!(
            answer.status, 409,
            "case {member_name:?}: a token replayed with a different {member_name} must be 409, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ConflictException",
            "case {member_name:?}: expected ConflictException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            "REVIEW_TEMPLATE",
            "case {member_name:?}: expected ResourceType REVIEW_TEMPLATE, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            arn,
            "case {member_name:?}: the conflict must name the template the token created, got {answer:?}"
        );
    }

    // A fresh token with the same parameters is a second, distinct template.
    let mut second_request = original.clone();
    second_request["ClientRequestToken"] = json!("tok-idempotent-2");
    let second = client.create_review_template(second_request).await;
    let second_arn = created_arn(&second, "fresh-token");
    assert_ne!(
        second_arn, arn,
        "case \"fresh-token\": a fresh token must create a second template"
    );
}

/// Rule 3. The rows differ along the ARN that resolves nothing: a never-minted id, an
/// id that differs from a stored one in a single character, and the same template id in
/// another Region — every one the same 404, naming the ARN exactly as sent.
#[tokio::test]
async fn a_template_arn_that_names_no_template_is_a_not_found() {
    let client = Client::new();
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

    let cases: Vec<(&str, String)> = vec![
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

    for (case, template_arn) in cases {
        assert_ne!(
            template_arn, present,
            "case {case:?}: the row must not name the template that exists"
        );
        let answer = client.get_review_template(&template_arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            "REVIEW_TEMPLATE",
            "case {case:?}: expected ResourceType REVIEW_TEMPLATE, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            template_arn,
            "case {case:?}: ResourceId must be the ARN as sent, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Message"),
            format!(
                "[ResourceNotFound] No review template with ARN {template_arn} was found in \
                 account {}.",
                engine::emulator_account_id()
            ),
            "case {case:?}: expected the ARN/account not-found message, got {answer:?}"
        );
    }

    // The template that DOES exist still resolves, so the rows above fail on resolution
    // and not on a broken route.
    let fetched = client.get_review_template(&present).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored template must still resolve, got {fetched:?}"
    );
}

/// Rule 4: "If a review template that specifies lenses is applied to the workload, those
/// lenses are applied to the workload in addition to these lenses"
/// (`CreateWorkloadInput$Lenses` doc), and "For a review template, these are the notes
/// that will be associated with the workload when the template is applied" (`Notes`
/// doc). The rows differ along how the two lens sets overlap, how each lens is named,
/// and whether the workload request brings notes of its own.
#[tokio::test]
async fn a_workload_takes_the_lenses_of_the_review_template_applied_to_it() {
    // One row of the table: the template to create, the workload to apply it to, and
    // what the workload must end up carrying.
    struct Case {
        case: &'static str,
        template_lenses: Value,
        template_notes: Option<&'static str>,
        workload_lenses: Value,
        workload_notes: Option<&'static str>,
        expected_lenses: Vec<&'static str>,
        expected_notes: Option<&'static str>,
    }

    let cases: Vec<Case> = vec![
        Case {
            case: "template-lens-not-on-the-workload",
            template_lenses: json!(["serverless"]),
            template_notes: None,
            workload_lenses: json!(["wellarchitected"]),
            workload_notes: None,
            expected_lenses: vec!["wellarchitected", "serverless"],
            expected_notes: None,
        },
        Case {
            case: "template-lens-already-on-the-workload",
            template_lenses: json!(["wellarchitected"]),
            template_notes: None,
            workload_lenses: json!(["wellarchitected"]),
            workload_notes: None,
            expected_lenses: vec!["wellarchitected"],
            expected_notes: None,
        },
        Case {
            case: "same-lens-named-by-arn-on-the-template",
            template_lenses: json!(["arn:aws:wellarchitected:us-east-1::lens/wellarchitected"]),
            template_notes: None,
            workload_lenses: json!(["wellarchitected"]),
            workload_notes: None,
            expected_lenses: vec!["wellarchitected"],
            expected_notes: None,
        },
        Case {
            case: "two-template-lenses-one-of-them-new",
            template_lenses: json!(["wellarchitected", "serverless"]),
            template_notes: None,
            workload_lenses: json!(["wellarchitected"]),
            workload_notes: None,
            expected_lenses: vec!["wellarchitected", "serverless"],
            expected_notes: None,
        },
        Case {
            case: "template-without-lenses",
            template_lenses: json!([]),
            template_notes: None,
            workload_lenses: json!(["wellarchitected"]),
            workload_notes: None,
            expected_lenses: vec!["wellarchitected"],
            expected_notes: None,
        },
        Case {
            case: "template-notes-carry-over",
            template_lenses: json!(["wellarchitected"]),
            template_notes: Some("notes from the template"),
            workload_lenses: json!(["wellarchitected"]),
            workload_notes: None,
            expected_lenses: vec!["wellarchitected"],
            expected_notes: Some("notes from the template"),
        },
        Case {
            case: "workload-notes-win-over-the-template",
            template_lenses: json!(["wellarchitected"]),
            template_notes: Some("notes from the template"),
            workload_lenses: json!(["wellarchitected"]),
            workload_notes: Some("notes from the request"),
            expected_lenses: vec!["wellarchitected"],
            expected_notes: Some("notes from the request"),
        },
    ];

    for row in cases {
        let Case {
            case,
            template_lenses,
            template_notes,
            workload_lenses,
            workload_notes,
            expected_lenses,
            expected_notes,
        } = row;

        let client = Client::new();
        let mut template_request = minimal_template(&format!("tpl-{case}"), &format!("tok-{case}"));
        template_request["Lenses"] = template_lenses.clone();
        if let Some(notes) = template_notes {
            template_request["Notes"] = json!(notes);
        }
        let template_arn = created_arn(&client.create_review_template(template_request).await, case);

        let mut workload_request = minimal_create(&format!("wl-{case}"), &format!("wl-tok-{case}"));
        workload_request["Lenses"] = workload_lenses.clone();
        workload_request["ReviewTemplateArns"] = json!([template_arn]);
        if let Some(notes) = workload_notes {
            workload_request["Notes"] = json!(notes);
        }
        let workload_id = created_id(&client.create(workload_request).await, case);

        let fetched = client.get(&workload_id).await;
        let workload = fetched
            .body
            .get("Workload")
            .cloned()
            .unwrap_or(Value::Null);
        let expected_lenses = json!(expected_lenses);
        assert_eq!(
            workload.get("Lenses"),
            Some(&expected_lenses),
            "case {case:?}: expected the workload's lenses to be {expected_lenses:?}, got {workload:?}"
        );
        assert_eq!(
            workload.get("Notes").and_then(Value::as_str),
            expected_notes,
            "case {case:?}: expected the workload's notes to be {expected_notes:?}, got {workload:?}"
        );
    }
}

/// Rule 4, the other half: the templates applied to a workload are part of the request
/// the workload's `ClientRequestToken` identifies — their lenses and notes land in the
/// stored workload — so a token replayed with the same template returns the original
/// workload and one replayed with a DIFFERENT template is the modelled
/// `ConflictException`.
#[tokio::test]
async fn the_applied_review_template_is_part_of_the_workload_create_request() {
    let client = Client::new();
    let first = created_arn(
        &client
            .create_review_template(minimal_template("tpl-applied-one", "tok-tpl-one"))
            .await,
        "tpl-one",
    );
    let second = created_arn(
        &client
            .create_review_template(minimal_template("tpl-applied-two", "tok-tpl-two"))
            .await,
        "tpl-two",
    );

    let mut request = minimal_create("workload-from-a-template", "wl-tok-applied");
    request["ReviewTemplateArns"] = json!([first]);
    let workload_id = created_id(&client.create(request.clone()).await, "first-create");

    let replay = client.create(request.clone()).await;
    assert_eq!(
        replay.body.get("WorkloadId").and_then(Value::as_str),
        Some(workload_id.as_str()),
        "case \"replay\": the same token with the same template must return the original workload, got {replay:?}"
    );

    for (case, arns) in [
        ("a-different-template", json!([second])),
        ("no-template-at-all", json!([])),
    ] {
        let mut diverged = request.clone();
        diverged["ReviewTemplateArns"] = arns;
        let answer = client.create(diverged).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: a token replayed with {case} must be 409, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {answer:?}"
        );
    }
}

