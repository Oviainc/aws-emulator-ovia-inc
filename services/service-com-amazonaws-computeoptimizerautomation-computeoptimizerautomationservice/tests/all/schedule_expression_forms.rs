// SPDX-License-Identifier: CC0-1.0

//! The FORMS a schedule expression may be written in, on every path that stores
//! one.
//!
//! The rule under test: a `schedule.scheduleExpression` is judged on the cron
//! expression it carries — after the optional `cron(...)` wrapper a schedule
//! expression names its kind with is removed — and that expression says when the
//! rule runs when it has the documented fields (`minutes hours day_of_month
//! month day_of_week`, with the trailing `year` optional, as the member's own
//! example `(30 12 * * *)` is written) and its minutes and hours fields each
//! name one specific in-range value
//! (`smithy_query(query="doc",shape="Schedule",member="scheduleExpression")`).
//! So the wrapper, the padding inside it and the case of its keyword change
//! nothing: an expression is accepted bare exactly when it is accepted wrapped,
//! and a form that is not a cron expression at all is refused
//! `InvalidParameterValueException` (HTTP 400).
//!
//! The cases are a TABLE: every cron body is crossed with every form it can be
//! written in, and each cell is asserted on the create path and on the update
//! path (which shares the same validation), with the stored value read back
//! through `GetAutomationRule`.
//!
//! Every case drives the service over the wire through the shared awsJson1_0
//! harness in [`crate::automation_rule_lifecycle`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, member, minimal_rule, string_member,
};

/// How a schedule expression can be WRITTEN, for one and the same cron body.
#[derive(Debug, Clone, Copy)]
enum Form {
    /// The cron expression on its own.
    Bare,
    /// The `cron(...)` function form a schedule expression names its kind with.
    Wrapped,
    /// The same wrapper, with whitespace around the whole value and inside the
    /// parentheses.
    WrappedPadded,
    /// The same wrapper, keyword in upper case: a keyword is not case
    /// sensitive.
    WrappedUpperKeyword,
}

/// Every form, so each cron body is asserted in all of them.
const FORMS: &[Form] = &[
    Form::Bare,
    Form::Wrapped,
    Form::WrappedPadded,
    Form::WrappedUpperKeyword,
];

impl Form {
    /// A label safe to build a `RuleName` out of (`^[a-zA-Z0-9_-]*$`).
    fn label(self) -> &'static str {
        match self {
            Form::Bare => "bare",
            Form::Wrapped => "wrapped",
            Form::WrappedPadded => "wrapped-padded",
            Form::WrappedUpperKeyword => "wrapped-upper-keyword",
        }
    }

    /// `body` written in this form.
    fn render(self, body: &str) -> String {
        match self {
            Form::Bare => body.to_string(),
            Form::Wrapped => format!("cron({body})"),
            Form::WrappedPadded => format!("  cron(  {body}  )  "),
            Form::WrappedUpperKeyword => format!("CRON({body})"),
        }
    }
}

/// The cron bodies the table walks, and whether the service accepts a schedule
/// carrying one: a specific minute and hour at both ends of their ranges, the
/// six-field form and the five-field form of the member's own example, each way
/// a cron field can stand for more than one value (wildcard, list, range,
/// step), a value past each field's bound, and a field count outside the
/// documented ones.
const CRON_BODIES: &[(&str, bool)] = &[
    // Accepted: specific minutes and hours, six fields.
    ("30 12 * * ? *", true),
    ("0 0 * * ? *", true),
    ("59 23 ? * MON *", true),
    ("15 6 1 1 ? 2027", true),
    ("07 09 * * ? *", true),
    // Accepted: the five-field form the member's own example is written in
    // ("For example: (30 12 * * *) runs daily at 12:30 PM UTC").
    ("30 12 * * *", true),
    ("0 23 1 1 *", true),
    // Refused: the minutes or hours field does not name ONE value.
    ("* 12 * * ? *", false),
    ("30 * * * ? *", false),
    ("? 12 * * ? *", false),
    ("0,30 12 * * ? *", false),
    ("30 0-6 * * ? *", false),
    ("*/15 12 * * ? *", false),
    ("30 12/2 * * ? *", false),
    ("+5 12 * * ? *", false),
    // Refused: past the field's bound.
    ("60 12 * * ? *", false),
    ("30 24 * * ? *", false),
    // Refused: a field count the documented expression does not have.
    ("30", false),
    ("30 12", false),
    ("30 12 * *", false),
    ("30 12 * * ? * *", false),
    // Refused: no expression at all (and, wrapped, an empty wrapper).
    ("", false),
    ("   ", false),
];

/// Values that are not a cron expression however they are read, so no path
/// stores them: another expression kind, an unterminated wrapper and a wrapper
/// whose keyword is not `cron`.
const NON_CRON_EXPRESSIONS: &[&str] = &[
    "rate(1 day)",
    "at(2027-01-01T00:00:00)",
    "cron(30 12 * * ? *",
    "crond(30 12 * * ? *)",
    "cron 30 12 * * ? *",
    // The parenthesis of the wrapper follows the keyword directly, so a gap
    // leaves a value that is not the wrapped form either.
    "cron (30 12 * * ? *)",
    "(30 12 * * ? *)",
];

/// The schedule a case sends: only the expression varies, so a refusal can come
/// from nothing else.
fn schedule(expression: &str) -> Value {
    json!({
        "scheduleExpression": expression,
        "scheduleExpressionTimezone": "UTC",
        "executionWindowInMinutes": 60,
    })
}

/// The rule every update case is applied to, and the revision it stands at.
struct Subject {
    arn: String,
    revision: i64,
}

impl Subject {
    async fn create(router: &MultiServiceRouter, name: &str) -> Self {
        let created = call_ok(router, "CreateAutomationRule", &minimal_rule(name), name).await;
        Self {
            arn: string_member(&created, "ruleArn"),
            revision: member(&created, "ruleRevision")
                .as_i64()
                .expect("the create reported a revision"),
        }
    }

    /// What `GetAutomationRule` reports for this rule right now.
    async fn read(&self, router: &MultiServiceRouter, case: &str) -> Value {
        call_ok(
            router,
            "GetAutomationRule",
            &json!({"ruleArn": &self.arn}),
            case,
        )
        .await
    }
}

/// Assert one expression on both write paths: accepted means the create stores
/// it and reads it back, and the update applies it and bumps the revision;
/// refused means HTTP 400 `InvalidParameterValueException` from both, with the
/// subject rule left where it was.
async fn assert_expression(
    router: &MultiServiceRouter,
    subject: &mut Subject,
    expression: &str,
    accepted: bool,
    case: &str,
) {
    let expected = schedule(expression);

    let mut request = minimal_rule(&format!("create-{case}"));
    request["schedule"] = expected.clone();
    let created = call(router, "CreateAutomationRule", &request).await;

    let update = json!({
        "ruleArn": &subject.arn,
        "ruleRevision": subject.revision,
        "schedule": expected.clone(),
    });
    let updated = call(router, "UpdateAutomationRule", &update).await;

    if accepted {
        assert_eq!(
            created.0, 200,
            "case {case:?}: expected the create of schedule expression {expression:?} to be \
             accepted with HTTP 200, got {} and {}",
            created.0, created.1
        );
        let fetched = call_ok(
            router,
            "GetAutomationRule",
            &json!({"ruleArn": string_member(&created.1, "ruleArn")}),
            case,
        )
        .await;
        assert_eq!(
            member(&fetched, "schedule"),
            &expected,
            "case {case:?}: the created rule must store schedule expression {expression:?} as \
             sent, got {fetched}"
        );

        assert_eq!(
            updated.0, 200,
            "case {case:?}: expected the update to schedule expression {expression:?} to be \
             accepted with HTTP 200, got {} and {}",
            updated.0, updated.1
        );
        subject.revision += 1;
        assert_eq!(
            member(&updated.1, "ruleRevision").as_i64(),
            Some(subject.revision),
            "case {case:?}: an applied update bumps the revision by one, got {}",
            updated.1
        );
        let after = subject.read(router, case).await;
        assert_eq!(
            member(&after, "schedule"),
            &expected,
            "case {case:?}: the updated rule must store schedule expression {expression:?} as \
             sent, got {after}"
        );
    } else {
        assert_refused(created, 400, "InvalidParameterValueException", case);
        assert_refused(updated, 400, "InvalidParameterValueException", case);
        let after = subject.read(router, case).await;
        assert_eq!(
            member(&after, "ruleRevision").as_i64(),
            Some(subject.revision),
            "case {case:?}: a refused update of schedule expression {expression:?} must leave the \
             rule's revision alone, got {after}"
        );
    }
}

/// One cron expression is judged the same whichever of its forms carries it:
/// bare, `cron(...)`-wrapped, padded inside the wrapper, or with the keyword in
/// another case.
#[tokio::test]
async fn a_cron_expression_is_judged_the_same_in_every_form_it_is_written_in() {
    let router = emulator();
    let mut subject = Subject::create(&router, "schedule-form-subject").await;

    for (index, (body, accepted)) in CRON_BODIES.iter().enumerate() {
        for form in FORMS {
            let expression = form.render(body);
            let case = format!("{}-{index}", form.label());
            assert_expression(&router, &mut subject, &expression, *accepted, &case).await;
        }
    }
}

/// A value that is not a cron expression says nothing about when a rule runs,
/// so no write path stores it.
#[tokio::test]
async fn a_value_that_is_not_a_cron_expression_is_refused() {
    let router = emulator();
    let mut subject = Subject::create(&router, "non-cron-subject").await;

    for (index, expression) in NON_CRON_EXPRESSIONS.iter().enumerate() {
        let case = format!("non-cron-{index}");
        assert_expression(&router, &mut subject, expression, false, &case).await;
    }
}
