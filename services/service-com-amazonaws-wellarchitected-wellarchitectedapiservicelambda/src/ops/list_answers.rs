// SPDX-License-Identifier: CC0-1.0

//! @op ListAnswers
//! This operation reports the answers of ONE lens review of ONE workload: the question
//! set of the lens, each question carrying the answer the workload records for it.
//!
//! It is a REPORTING operation — it resolves the workload, the milestone (when one is
//! named) and the lens review, reads the lens's question catalog and the workload's
//! stored answers, and writes nothing.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListAnswers`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! List of answers for a particular workload and lens.
//!
//! ## Input
//!
//! * `LensAlias` (LensAlias, len 1..128) **required** — The alias of the lens. For Amazon Web
//!   Services official lenses, this is either the lens alias, such as serverless, or the lens
//!   ARN, such as arn:aws:wellarchitected:us-east-1::lens/serverless. Note that some
//!   operations (such as ExportLens and CreateLensShare) are not permitted on Amazon Web
//!   Services official lenses. For custom lenses, this is the lens ARN, such as
//!   arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef.
//!   Each lens is identified by its LensSummary$LensAlias.
//! * `MaxResults` (MaxResults) — The maximum number of results to return for this request.
//! * `MilestoneNumber` (MilestoneNumber, range 1..100) — The milestone number. A workload can
//!   have a maximum of 100 milestones.
//! * `NextToken` (NextToken, pattern ^[A-Za-z0-9+\/=_-]+$) — The token to use to retrieve the
//!   next set of results.
//! * `PillarId` (PillarId, len 1..64) — The ID used to identify a pillar, for example,
//!   security. A pillar is identified by its PillarReviewSummary$PillarId.
//! * `QuestionPriority` (QuestionPriority, one of: NONE | PRIORITIZED) — The priority of the
//!   question.
//! * `WorkloadId` (WorkloadId, len 32..32, pattern ^[0-9a-f]{32}$) **required** — The ID
//!   assigned to the workload. This ID is unique within an Amazon Web Services Region.
//!
//! ## Output
//!
//! * `AnswerSummaries` (AnswerSummaries) — List of answer summaries of lens review in a
//!   workload.
//! * `LensAlias` (LensAlias, len 1..128) — The alias of the lens. For Amazon Web Services
//!   official lenses, this is either the lens alias, such as serverless, or the lens ARN,
//!   such as arn:aws:wellarchitected:us-east-1::lens/serverless. Note that some operations
//!   (such as ExportLens and CreateLensShare) are not permitted on Amazon Web Services
//!   official lenses. For custom lenses, this is the lens ARN, such as
//!   arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef.
//!   Each lens is identified by its LensSummary$LensAlias.
//! * `LensArn` (LensArn) — The ARN for the lens.
//! * `MilestoneNumber` (MilestoneNumber, range 1..100) — The milestone number. A workload can
//!   have a maximum of 100 milestones.
//! * `NextToken` (NextToken, pattern ^[A-Za-z0-9+\/=_-]+$) — The token to use to retrieve the
//!   next set of results.
//! * `WorkloadId` (WorkloadId, len 32..32, pattern ^[0-9a-f]{32}$) — The ID assigned to the
//!   workload. This ID is unique within an Amazon Web Services Region.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `InternalServerException` (HTTP 500) — There is a problem with the Well-Architected Tool
//!   API service.
//! * `ResourceNotFoundException` (HTTP 404) — The requested resource was not found.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The user input is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListAnswersError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAnswersError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#ListAnswers")]
#[derive(Debug)]
pub enum ListAnswersErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_answers::ListAnswersError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAnswersError = crate::scaffolded::Scaffolded<ListAnswersErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAnswersErrorModeled> for ListAnswersError {
    fn from(modeled: ListAnswersErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAnswers` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAnswers<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAnswers<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAnswers<Eng>
{
    type Input = ListAnswersInput;
    type Output = ListAnswersOutput;
    type Error = ListAnswersError;

    const NAME: &'static str = "ListAnswers";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ANSWERS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_ANSWERS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListAnswersErrorModeled::InternalServerException(InternalServerException {
                message: "the Well-Architected Tool workload store is unavailable".to_string(),
            })
        })?;

        // `MaxResults` is modelled `@range(min: 1, max: 50)`; a request that omits it asks
        // for a full page, so the modelled maximum is the default. One helper restates that
        // bound for every paginated operation of this service.
        let page_size = crate::state::page_size(input.max_results);

        // An opaque token this service did not issue is a bad request, not an empty page.
        // A token is this service's own cursor, not a value a caller composes: it cannot
        // be corrected, only obtained again, so the refusal is classified `CANNOT_PARSE`
        // ("restart the pagination") rather than as a field rule to fix — and it still
        // names the member whose value could not be decoded.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_question_token(token) {
                Some(question_id) => Some(question_id),
                None => return Err(invalid_next_token()),
            },
        };

        let store = state.lock();
        let live = store
            .workloads
            .get(&input.workload_id)
            .ok_or_else(|| {
                ListAnswersErrorModeled::ResourceNotFoundException(
                    crate::state::workload_not_found(&input.workload_id),
                )
            })?;

        // "A milestone records the state of a workload at a particular point in time"
        // (`CreateMilestone` doc): a request that names one reads the answers the
        // RECORDED workload carried, not the live ones.
        let workload = match input.milestone_number {
            None => live,
            Some(number) => {
                &store
                    .milestone(&input.workload_id, number)
                    .ok_or_else(|| {
                        ListAnswersErrorModeled::ResourceNotFoundException(
                            crate::state::milestone_not_found(&input.workload_id, number),
                        )
                    })?
                    .workload
            }
        };

        // The answers of a lens review exist only while that lens is associated with the
        // workload, so a lens the workload does not carry has no review to report.
        if !workload
            .lenses
            .iter()
            .any(|lens| crate::state::lens_alias(lens) == crate::state::lens_alias(&input.lens_alias))
        {
            return Err(ListAnswersErrorModeled::ResourceNotFoundException(
                crate::state::lens_review_not_found(&input.workload_id, &input.lens_alias),
            )
            .into());
        }

        // The lens's questions, in the lens's own order, narrowed by the request's
        // filters and paged through the service's shared question cursor. `PillarId` and
        // `QuestionPriority` FILTER the question set rather than resolving anything, so
        // a value that matches no question is an empty page.
        //
        // Every question of a workload lens review here carries
        // `crate::state::DEFAULT_QUESTION_TYPE`, so the priority filter selects all of
        // them or none.
        let priority_selects = input.question_priority.as_deref().is_none_or(|priority| {
            priority == crate::state::question_priority_of(crate::state::DEFAULT_QUESTION_TYPE)
        });
        let page = crate::state::question_page(
            crate::state::lens_questions(&input.lens_alias),
            |question| {
                priority_selects
                    && input
                        .pillar_id
                        .as_deref()
                        .is_none_or(|pillar| pillar == question.pillar_id)
            },
            resume_after.as_deref(),
            page_size,
        )
        .ok_or_else(invalid_next_token)?;

        let answer_summaries = page
            .questions
            .iter()
            .map(|question| {
                answer_summary(question, workload.answer(&input.lens_alias, question.question_id))
            })
            .collect();

        Ok(ListAnswersOutput {
            workload_id: Some(workload.workload_id.clone()),
            // Echoed only when the request scoped the read to a milestone: a read of the
            // live review is not a read of any milestone.
            milestone_number: input.milestone_number,
            // The lens as the request named it, beside the ARN it resolves to: an
            // AWS-official lens's ARN carries no owner account and is built from the
            // alias, while a custom lens's is the one its import minted and the lens
            // store holds. (A custom lens cannot be reviewed until `CreateLensVersion`
            // publishes it — its own stub — so a custom lens named in a workload's
            // `Lenses` is reported by the ARN the caller named it with.)
            lens_alias: Some(input.lens_alias.clone()),
            lens_arn: Some(if crate::state::is_aws_official_lens(&input.lens_alias) {
                crate::state::official_lens_arn(
                    &crate::state::request_region(ctx.parts()),
                    &input.lens_alias,
                )
            } else {
                store
                    .lens_by_alias(&input.lens_alias)
                    .map_or_else(|| input.lens_alias.clone(), |lens| lens.lens_arn.clone())
            }),
            answer_summaries: Some(answer_summaries),
            next_token: page.next_token,
        })
    }
}

/// The `AnswerSummary` for one question of a lens review: the question as the lens
/// defines it, plus the answer the workload records for it.
///
/// A question with NO stored answer is `UNANSWERED` and applicable, with nothing
/// selected — that is what an unanswered question of an attached lens means, and it is
/// also what the workload's risk roll-up counts it as.
fn answer_summary(
    question: &crate::state::FrameworkQuestion,
    answer: Option<&crate::state::AnswerRecord>,
) -> AnswerSummary {
    AnswerSummary {
        question_id: Some(question.question_id.to_string()),
        pillar_id: Some(question.pillar_id.to_string()),
        question_title: Some(question.title.to_string()),
        // The per-question CHOICES a lens offers are lens content this emulator does
        // not ship (see `crate::state::FRAMEWORK_LENS_QUESTIONS`), and the member is
        // optional in the model, so no catalog of choices is reported. The choices an
        // ANSWER speaks about are reported below, from the answer itself.
        choices: None,
        selected_choices: Some(
            answer.map(|answer| answer.selected_choices.clone()).unwrap_or_default(),
        ),
        choice_answer_summaries: Some(
            answer
                .map(|answer| {
                    answer
                        .choice_answers
                        .iter()
                        .map(|(choice_id, choice)| ChoiceAnswerSummary {
                            choice_id: Some(choice_id.clone()),
                            status: Some(choice.status.clone()),
                            reason: choice.reason.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        // A question applies unless an answer says it does not.
        is_applicable: Some(answer.is_none_or(|answer| answer.is_applicable)),
        risk: Some(
            answer
                .map_or(crate::state::RISK_UNANSWERED, crate::state::AnswerRecord::risk)
                .to_string(),
        ),
        // `Reason` exists only once an answer gives one.
        reason: answer.and_then(|answer| answer.reason.clone()),
        question_type: Some(crate::state::DEFAULT_QUESTION_TYPE.to_string()),
        // A question's Jira issue exists once the workload's Jira integration has
        // synced one; `UpdateIntegration` is still a stub, so nothing is synced here.
        jira_configuration: None,
    }
}

/// What a `NextToken` this service did not issue is answered with. The token is this
/// service's own opaque cursor, so the only thing a caller can do is start the
/// pagination again — which is what `CANNOT_PARSE` tells it.
fn invalid_next_token() -> ListAnswersError {
    ListAnswersErrorModeled::ValidationException(crate::state::cannot_parse_exception(
        crate::state::NEXT_TOKEN_MEMBER,
        crate::state::INVALID_NEXT_TOKEN_MESSAGE,
    ))
    .into()
}
