// SPDX-License-Identifier: CC0-1.0

//! @op PutComplianceInquiryFeedback
//! Records the caller's rating of a compliance inquiry's response and answers the instant
//! the feedback was submitted.
//!
//! Five rules shape the handler:
//!
//! * it writes onto a PERSISTED inquiry: an id the store does not hold names no inquiry
//!   this account may give feedback on, which the front door answers with its 401 — the
//!   same refusal `GetComplianceInquiryMetadata` and `ExportComplianceInquiry` give for
//!   that id, rather than a 404 confirming the resource's absence;
//! * `queryIdentifier` names WHICH of the inquiry's queries the feedback is on, so an
//!   identifier that names no query of that inquiry is refused with the modelled
//!   `ValidationException` naming the member and the requirement it failed — the same
//!   refusal `ExportComplianceInquiry` gives its own `queryIdentifiers`; naming none
//!   records the feedback on the inquiry's response as a whole;
//! * a QUESTION-LEVEL submission — one that names a `queryIdentifier` — is feedback on a
//!   VERSION of that query's response, so it must name the version it is about:
//!   `responseRevisionId` is what identifies that version ("use this value to prevent
//!   submitting feedback on a stale response"), and a question-level submission carrying
//!   none is refused with the modelled `ValidationException`
//!   ([`crate::state::question_level_revision_required`]) rather than attributed to
//!   whichever revision happens to be current. Feedback on the inquiry's response as a
//!   whole names no query, so it names no revision of one either and the requirement does
//!   not apply;
//! * the feedback RECORD is unique per response revision: `queryIdentifier` together with
//!   `responseRevisionId` names the response version being rated
//!   ([`crate::state::FeedbackTarget`]), that version accepts exactly ONE submitted
//!   feedback, and a later submission against a version that already carries feedback is
//!   refused with the modelled `ConflictException` — whatever `clientToken` it carries.
//!   That is the guard `responseRevisionId` advertises ("use this value to prevent
//!   submitting feedback on a stale response"): the refusal is a property of the targeted
//!   version's STATE, not of a token. Feedback on another version of the same query, or on
//!   another query, targets a version of its own and is recorded;
//! * `clientToken` is this operation's `@idempotencyToken`, and what the model says a token
//!   this service has already recorded does, it says without exception: "If this token
//!   matches a previous request, the service ignores the request, but does not return an
//!   error". So a request carrying a remembered token is IGNORED rather than refused for
//!   carrying it — nothing further is recorded — and answered with the `submittedAt` the
//!   call that recorded the token answered, however the rest of that request differs from
//!   it: NO refusal of this operation is keyed on a token, and what can still refuse such a
//!   request is what the request itself carried (the two rules above) or the targeted
//!   version's own state. A RETRY (the token on
//!   the very request it was recorded for) is ignored before that state rule, because the
//!   record it would conflict with is the one it already made; the same token on another
//!   request is ignored after it, because the rule judges the targeted version's state
//!   whatever token reached it. `CreateComplianceInquiry`'s own `clientToken` carries no
//!   such sentence ("Idempotency token for the request."), so its conflict on a token
//!   reused for a different create stands.
//!
//! `responseRevisionId` is not compared against a GENERATED revision: nothing here
//! produces a response or an edit to one (see [`crate::state::InquiryQuery::response`]), so
//! there is no stored revision a request's value could be stale against, and a staleness
//! refusal would be a comparison against a revision nothing produced. The value is what
//! identifies the rated version, and that is what the one-feedback-per-version rule is keyed
//! on.
//!
//! What the call records is observable: the document `ExportComplianceInquiry` renders
//! reports the feedback held for the queries it covers.
//!
//! The submission changes persisted state, so it is ATTRIBUTABLE on the same terms as this
//! crate's other state changes: this handler leaves exactly one structured record naming the
//! caller (the access-key id and the credential-scope region the request presented), what the
//! call addressed and the outcome — on the refusal and replay paths as much as on the success
//! one, since a state-changing call that changed nothing is still attributable. Never the
//! `comment`, nor the inquiry's name, content or queries, all `@sensitive` in the model.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `PutComplianceInquiryFeedback::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
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
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutComplianceInquiryFeedback`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json")
//! ```
//!
//! Submits feedback on a compliance inquiry response.
//!
//! ## Input
//!
//! * `clientToken` (IdempotentClientToken, len 1..128, pattern ^[\w\-]+$, @idempotencyToken)
//!   — A unique, case-sensitive identifier to ensure that the operation completes no more
//!   than one time. If this token matches a previous request, the service ignores the
//!   request, but does not return an error.
//! * `comment` (FeedbackCommentAttribute, len 1..1000, pattern
//!   ^[a-zA-Z0-9\s.,;:!?'\-()/@#&+=]+$) — An optional comment for the feedback.
//! * `complianceInquiryId` (InquiryId, pattern ^compliance-inquiry-[a-zA-Z0-9]{16}$)
//!   **required** — The unique identifier for the compliance inquiry.
//! * `queryIdentifier` (Integer) — The sequential identifier of the query to provide feedback
//!   on.
//! * `rating` (FeedbackRating, one of: THUMBS_DOWN | THUMBS_UP) **required** — The rating for
//!   the feedback. Valid values are THUMBS_UP and THUMBS_DOWN.
//! * `reasonCodes` (FeedbackReasonCodeList) — The reason codes that describe why you rated
//!   the response. Valid values are OTHER, PARTIAL_RESPONSE, and IRRELEVANT_RESPONSE.
//! * `responseRevisionId` (Integer) — The response revision ID. Use this value to prevent
//!   submitting feedback on a stale response.
//!
//! ## Output
//!
//! * `submittedAt` (TimestampAttribute) **required** — The timestamp when the feedback was
//!   submitted.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — Request to create/modify content would result in a
//!   conflict.
//! * `InternalServerException` (HTTP 500) — An unknown server exception has occurred.
//! * `ResourceNotFoundException` (HTTP 404) — Request references a resource which does not
//!   exist.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — Request fails to satisfy the constraints specified by
//!   an AWS service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutComplianceInquiryFeedbackError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-artifact-artifact.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutComplianceInquiryFeedbackError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.artifact#PutComplianceInquiryFeedback")]
#[derive(Debug)]
pub enum PutComplianceInquiryFeedbackErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_compliance_inquiry_feedback::PutComplianceInquiryFeedbackError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutComplianceInquiryFeedbackError = crate::scaffolded::Scaffolded<PutComplianceInquiryFeedbackErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutComplianceInquiryFeedbackErrorModeled> for PutComplianceInquiryFeedbackError {
    fn from(modeled: PutComplianceInquiryFeedbackErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutComplianceInquiryFeedback` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutComplianceInquiryFeedback<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutComplianceInquiryFeedback<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutComplianceInquiryFeedback<Eng>
{
    type Input = PutComplianceInquiryFeedbackInput;
    type Output = PutComplianceInquiryFeedbackOutput;
    type Error = PutComplianceInquiryFeedbackError;

    const NAME: &'static str = "PutComplianceInquiryFeedback";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_COMPLIANCE_INQUIRY_FEEDBACK_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_COMPLIANCE_INQUIRY_FEEDBACK_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Nothing in the request metadata shapes the feedback this records; the request's
        // credential scope is read only to ATTRIBUTE the change below, exactly as the
        // account-wide write and the tag writes read it.
        let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
        let region = crate::state::request_region(ctx.parts());

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // A state-changing call that changed nothing is still attributable, so the
            // failure path records its outcome rather than passing silently.
            ::tracing::info!(
                target: "emulator::audit",
                operation = "PutComplianceInquiryFeedback",
                akid = %akid,
                region = %region,
                compliance_inquiry_id = %input.compliance_inquiry_id,
                outcome = "unavailable",
                "state-changing operation"
            );
            return Err(
                PutComplianceInquiryFeedbackErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "the artifact compliance inquiry store is unavailable".to_string(),
                        retry_after_seconds: None,
                    },
                )
                .into(),
            );
        };

        let now = ::std::time::SystemTime::now();

        // "The reason codes that describe why you rated the response": which reasons were
        // given is a SET, so the list the request carried is normalised the one way this
        // service normalises such a member ([`crate::state::request_set`]) — the same
        // treatment `ExportComplianceInquiry` gives its `queryIdentifiers`.
        let reason_codes = crate::state::request_set(input.reason_codes.clone().unwrap_or_default());

        // What this request's `clientToken` was recorded for: every member that shapes the
        // feedback recorded, so a RETRY of the request the token was recorded on can be
        // told from the same token sent on another request.
        let fingerprint = request_fingerprint(&input, &reason_codes);

        let mut store = state.lock();

        // Read before the inquiry is borrowed for the write. WHICH answer the caller sees
        // is decided below in the order the service decides it: a retry of the request this
        // token was recorded on is answered first, then the TARGET's state decides a
        // refusal, and a token recorded on another request is ignored after it.
        let remembered = input
            .client_token
            .as_deref()
            .and_then(|token| store.inquiry_feedback_tokens.get(token).cloned());

        // The one rule every inquiry-targeting operation of this service resolves its
        // target by, decided in `crate::state`: a well-formed id this account holds no
        // inquiry for names no inquiry it may rate, so the front door's refusal is what it
        // sees — not the modelled 404, which would confirm the resource's absence — and
        // the inquiry it does hold comes back with the only clock-owed write applied, the
        // transition this inquiry was already due. The refusal is handed back rather than
        // stated by the rule, which is what lets this attributable call record its outcome
        // before answering.
        let inquiry = match store.targeted_inquiry_mut(&input.compliance_inquiry_id, now) {
            Ok(inquiry) => inquiry,
            Err(refusal) => {
                ::tracing::info!(
                    target: "emulator::audit",
                    operation = "PutComplianceInquiryFeedback",
                    akid = %akid,
                    region = %region,
                    compliance_inquiry_id = %input.compliance_inquiry_id,
                    outcome = "refused",
                    reason = "complianceInquiryId names no inquiry this account holds",
                    "state-changing operation"
                );
                return Err(crate::scaffolded::Scaffolded::FrontDoor(refusal));
            }
        };

        // "The sequential identifier of the query to provide feedback on": a query of THIS
        // inquiry, so an identifier it does not own is refused with the modelled
        // `ValidationException` naming the member. The ownership rule is asked of the
        // inquiry itself ([`crate::state::ComplianceInquiry::owns_query`]) — the same
        // predicate `ExportComplianceInquiry` refuses its `queryIdentifiers` against.
        if let Some(identifier) = input.query_identifier {
            if !inquiry.owns_query(identifier) {
                ::tracing::info!(
                    target: "emulator::audit",
                    operation = "PutComplianceInquiryFeedback",
                    akid = %akid,
                    region = %region,
                    compliance_inquiry_id = %input.compliance_inquiry_id,
                    query_identifier = input.query_identifier,
                    outcome = "refused",
                    reason = "queryIdentifier names a query the inquiry does not own",
                    "state-changing operation"
                );
                return Err(PutComplianceInquiryFeedbackErrorModeled::ValidationException(
                    crate::state::unknown_query_identifiers("queryIdentifier"),
                )
                .into());
            }
        }

        // "The response revision ID. Use this value to prevent submitting feedback on a
        // stale response": a submission scoped to ONE query is feedback on a VERSION of
        // that query's response, so it has to name which version it is about. A
        // question-level submission that names none is refused with the modelled
        // `ValidationException` ([`crate::state::question_level_revision_required`]) —
        // nothing is recorded, because recording it would attribute the rating to whatever
        // revision happened to be current, which is exactly the stale-response attribution
        // the member exists to prevent. Feedback on the inquiry's response as a WHOLE names
        // no query, so it names no revision of one either and the requirement does not
        // apply. Judged after the ownership rule above: which query the request named is
        // the first thing about a question-level submission.
        if input.query_identifier.is_some() && input.response_revision_id.is_none() {
            ::tracing::info!(
                target: "emulator::audit",
                operation = "PutComplianceInquiryFeedback",
                akid = %akid,
                region = %region,
                compliance_inquiry_id = %input.compliance_inquiry_id,
                query_identifier = input.query_identifier,
                outcome = "refused",
                reason = "a question-level submission named no response revision",
                "state-changing operation"
            );
            return Err(PutComplianceInquiryFeedbackErrorModeled::ValidationException(
                crate::state::question_level_revision_required(),
            )
            .into());
        }

        // "If this token matches a previous request, the service ignores the request, but
        // does not return an error". A RETRY — this token on the very request it was
        // recorded for — is answered here, before the state rule below, because the record
        // it would conflict with is the one it already made: nothing further is recorded and
        // the answer is the one that call gave, read off the remembered record. The retry is
        // still a call on a state-changing operation, so it is attributable: it recorded
        // nothing further, which is its outcome.
        if let Some(record) = &remembered {
            if record.fingerprint == fingerprint {
                ::tracing::info!(
                    target: "emulator::audit",
                    operation = "PutComplianceInquiryFeedback",
                    akid = %akid,
                    region = %region,
                    compliance_inquiry_id = %input.compliance_inquiry_id,
                    query_identifier = input.query_identifier,
                    response_revision_id = input.response_revision_id,
                    outcome = "replayed",
                    "state-changing operation"
                );
                return Ok(PutComplianceInquiryFeedbackOutput {
                    submitted_at: ::aws_smithy_types::DateTime::from(record.submitted_at),
                });
            }
        }

        // The RULE: the response version this request rates — the query it names together
        // with the revision of that query's response — accepts exactly one submitted
        // feedback, so a version this inquiry already holds feedback for is refused with the
        // modelled `ConflictException`. The refusal is keyed on the targeted version's
        // state, so no `clientToken` gets past it: a fresh one, none, or one this service
        // already recorded for another request.
        let target: crate::state::FeedbackTarget =
            (input.query_identifier, input.response_revision_id);
        if inquiry.feedback_for(target).is_some() {
            ::tracing::info!(
                target: "emulator::audit",
                operation = "PutComplianceInquiryFeedback",
                akid = %akid,
                region = %region,
                compliance_inquiry_id = %input.compliance_inquiry_id,
                query_identifier = input.query_identifier,
                response_revision_id = input.response_revision_id,
                outcome = "refused",
                reason = "the response version already carries feedback",
                "state-changing operation"
            );
            return Err(PutComplianceInquiryFeedbackErrorModeled::ConflictException(
                crate::state::feedback_already_submitted(&input.compliance_inquiry_id),
            )
            .into());
        }

        // The same sentence, for this token on a request it was NOT recorded for: it still
        // "matches a previous request", so this request is IGNORED as well — nothing is
        // recorded for it, whatever it asked for — and it is answered with the instant the
        // call that recorded the token answered. So no refusal of this operation is keyed on
        // a `clientToken`: only the targeted version's state above refuses a submission, and
        // it does so whatever token the request carried. Attributable like every other path:
        // the call recorded nothing, which is its outcome.
        if let Some(record) = &remembered {
            ::tracing::info!(
                target: "emulator::audit",
                operation = "PutComplianceInquiryFeedback",
                akid = %akid,
                region = %region,
                compliance_inquiry_id = %input.compliance_inquiry_id,
                query_identifier = input.query_identifier,
                response_revision_id = input.response_revision_id,
                outcome = "ignored",
                reason = "the clientToken was recorded for another request",
                "state-changing operation"
            );
            return Ok(PutComplianceInquiryFeedbackOutput {
                submitted_at: ::aws_smithy_types::DateTime::from(record.submitted_at),
            });
        }

        // Residency bound: one inquiry holds at most `FEEDBACK_PER_INQUIRY_CAPACITY`
        // records, and a request may name any revision an `Integer` carries, so repeated
        // submissions cannot grow the emulator's memory without end. Nothing is evicted — a
        // forgotten record would let a rated response version be rated again.
        if !inquiry.accepts_another_feedback() {
            ::tracing::info!(
                target: "emulator::audit",
                operation = "PutComplianceInquiryFeedback",
                akid = %akid,
                region = %region,
                compliance_inquiry_id = %input.compliance_inquiry_id,
                query_identifier = input.query_identifier,
                response_revision_id = input.response_revision_id,
                outcome = "refused",
                reason = "the inquiry holds as many feedback records as it may",
                "state-changing operation"
            );
            return Err(PutComplianceInquiryFeedbackErrorModeled::ThrottlingException(
                ThrottlingException {
                    message: format!(
                        "this emulator stores at most {} feedback records per compliance inquiry",
                        crate::state::FEEDBACK_PER_INQUIRY_CAPACITY
                    ),
                    service_code: Some("artifact".to_string()),
                    quota_code: Some("FeedbackPerComplianceInquiry".to_string()),
                    retry_after_seconds: None,
                },
            )
            .into());
        }

        // The first and only feedback this response version carries.
        let reason_code_count = reason_codes.len();
        inquiry.record_feedback(crate::state::InquiryFeedback {
            query_identifier: input.query_identifier,
            rating: input.rating.clone(),
            reason_codes,
            comment: input.comment.clone(),
            response_revision_id: input.response_revision_id,
            submitted_at: now,
        });

        if let Some(token) = input.client_token {
            store.record_inquiry_feedback_token(
                token,
                &input.compliance_inquiry_id,
                &fingerprint,
                now,
            );
        }

        // Attributable: the caller's access key id (a public identifier, never a secret),
        // the scope's region, the inquiry rated, which query, and how. NEVER the `comment`
        // — `@sensitive` in the model — nor the inquiry's name, content or queries.
        ::tracing::info!(
            target: "emulator::audit",
            operation = "PutComplianceInquiryFeedback",
            akid = %akid,
            region = %region,
            compliance_inquiry_id = %input.compliance_inquiry_id,
            query_identifier = input.query_identifier,
            response_revision_id = input.response_revision_id,
            rating = %input.rating,
            reason_code_count,
            outcome = "recorded",
            "state-changing operation"
        );

        Ok(PutComplianceInquiryFeedbackOutput {
            // "The timestamp when the feedback was submitted" — the instant the record
            // above carries, so the answer and the stored feedback cannot disagree.
            submitted_at: ::aws_smithy_types::DateTime::from(now),
        })
    }
}

/// A fingerprint of the feedback submission this request carries, so a RETRY of the request
/// a `clientToken` was recorded on can be told from the same token sent on another request.
///
/// The two are ignored at different points of the handler — a retry before the per-version
/// rule, because the record it would conflict with is its own, and a reuse after it, because
/// that rule is keyed on the targeted version's state — and neither is answered with an
/// error.
///
/// Every member that shapes the recorded feedback is absorbed into the shared
/// [`crate::state::RequestFingerprint`], which labels and length-prefixes each one so no
/// two different requests can produce the same digest by splitting their fields
/// differently. The reason codes are absorbed in the normalised order the record stores
/// them in, because their wire order is not part of the request.
fn request_fingerprint(
    input: &PutComplianceInquiryFeedbackInput,
    reason_codes: &[String],
) -> String {
    let mut fingerprint = crate::state::RequestFingerprint::new();
    fingerprint.absorb_str("complianceInquiryId", &input.compliance_inquiry_id);
    fingerprint.absorb_optional_i32("queryIdentifier", input.query_identifier);
    fingerprint.absorb_str("rating", &input.rating);
    fingerprint.absorb_optional_i32("responseRevisionId", input.response_revision_id);
    for code in reason_codes {
        fingerprint.absorb_str("reasonCode", code);
    }
    fingerprint.absorb_optional_str("comment", input.comment.as_deref());
    fingerprint.finish()
}
