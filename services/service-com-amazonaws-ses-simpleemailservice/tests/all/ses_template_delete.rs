// SPDX-License-Identifier: CC0-1.0

//! What a delete leaves behind for `UpdateTemplate`, end to end over the wire.
//!
//! `ses_template_deletion.rs` asserts the delete itself; this suite asserts the
//! ONE rule that spans the two operations, which neither suite can state alone:
//! an update replaces the content of a template the account HOLDS, so once a
//! template is removed the same update is the modelled absence instead — and a
//! create under the freed name makes it an update again, applying to the new
//! template and never to the content the removed one held.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind
//! it. The rows span the classes a template name and its content can be drawn
//! from; the removal is the same for each.

use crate::harness::{call, element, emulator, field, xml_text, Field};
use smithy_server_runtime::MultiServiceRouter;

/// One template as a request carries it.
#[derive(Debug, Clone)]
struct TemplateRow {
    case: &'static str,
    name: String,
    subject: String,
}

impl TemplateRow {
    fn new(case: &'static str, name: &str, subject: &str) -> Self {
        Self {
            case,
            name: name.to_string(),
            subject: subject.to_string(),
        }
    }

    /// The `Template.*` form fields for this row, as both `CreateTemplate` and
    /// `UpdateTemplate` take them.
    fn fields(&self) -> Vec<Field> {
        vec![
            field("Template.TemplateName", self.name.clone()),
            field("Template.SubjectPart", self.subject.clone()),
            field("Template.TextPart", format!("{}, as text.", self.subject)),
            field(
                "Template.HtmlPart",
                format!("<p>{}, as HTML.</p>", self.subject),
            ),
        ]
    }
}

/// One call carrying a whole `Template`: `(status, body)`.
async fn template_call(
    router: &MultiServiceRouter,
    action: &str,
    row: &TemplateRow,
) -> (u16, String) {
    call(router, action, &row.fields()).await
}

/// `CreateTemplate`, asserting it was accepted — the precondition, not what this
/// suite is about.
async fn create_template(router: &MultiServiceRouter, row: &TemplateRow) {
    let (status, body) = template_call(router, "CreateTemplate", row).await;
    assert_eq!(
        status, 200,
        "case {}: expected 200 from CreateTemplate, got {status} with {body}",
        row.case
    );
}

/// `DeleteTemplate`, asserting it was accepted.
async fn delete_template(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = call(router, "DeleteTemplate", &[field("TemplateName", name)]).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from DeleteTemplate({name:?}), got {status} with {body}"
    );
}

/// The subject line `GetTemplate` reports for `name`, or the `<Code>` of the
/// refusal it answered with.
async fn read_subject(router: &MultiServiceRouter, name: &str) -> Result<Option<String>, String> {
    let (status, body) = call(router, "GetTemplate", &[field("TemplateName", name)]).await;
    if status != 200 {
        return Err(element(&body, "Code").unwrap_or_else(|| format!("no error code in {body}")));
    }
    let template = element(&body, "Template")
        .unwrap_or_else(|| panic!("GetTemplate({name:?}) must report a Template, got {body}"));
    Ok(element(&template, "SubjectPart").map(|text| xml_text(&text)))
}

/// The templates the rule is applied to: the rows differ in the class of name
/// they carry and in the content they hold, so neither can decide the outcome.
fn rows() -> Vec<TemplateRow> {
    vec![
        TemplateRow::new("a plain name", "plain", "Your receipt"),
        TemplateRow::new("a name of digits", "0123456789", "Digits"),
        TemplateRow::new("a name with dashes and underscores", "a-name_with-both", "Mixed"),
        TemplateRow::new(
            "a name differing from another only by case",
            "PLAIN",
            "Upper case name",
        ),
        TemplateRow::new(
            "a name carrying non-ASCII text",
            "ünïcödé-名前",
            "Grüße, 你好",
        ),
        TemplateRow::new("a 64-character name", &"n".repeat(64), "A long name"),
        TemplateRow::new(
            "placeholder markup in the subject",
            "with-placeholders",
            "Hello {{name}}",
        ),
    ]
}

#[tokio::test]
async fn an_update_of_a_removed_template_is_the_modelled_absence_until_the_name_is_created_again() {
    let router = emulator();

    for row in rows() {
        // While the account holds the template, the same request is an update.
        create_template(&router, &row).await;
        let moved = TemplateRow {
            case: row.case,
            name: row.name.clone(),
            subject: format!("{} (updated)", row.subject),
        };
        let (status, body) = template_call(&router, "UpdateTemplate", &moved).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 updating a template the account holds, got {status} with {body}",
            row.case
        );
        assert_eq!(
            read_subject(&router, &row.name).await,
            Ok(Some(moved.subject.clone())),
            "case {}: the update must replace the stored content",
            row.case
        );

        // Once it is removed, the very same request is refused as the absence
        // the operation models, and records nothing.
        delete_template(&router, row.case, &row.name).await;
        let (status, body) = template_call(&router, "UpdateTemplate", &moved).await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400 updating a removed template, got {status} with {body}",
            row.case
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("TemplateDoesNotExist"),
            "case {}: expected the modelled TemplateDoesNotExist, got {body}",
            row.case
        );
        assert_eq!(
            element(&body, "Message")
                .map(|text| xml_text(&text))
                .as_deref(),
            Some(format!("Template {} does not exist.", row.name).as_str()),
            "case {}: expected the absent-template message naming {:?}, got {body}",
            row.case,
            row.name
        );
        assert_eq!(
            read_subject(&router, &row.name).await,
            Err("TemplateDoesNotExist".to_string()),
            "case {}: a refused update must record nothing under a removed name",
            row.case
        );

        // Created again, the name holds the NEW content, and an update applies to
        // that: nothing of the removed template survives.
        let recreated = TemplateRow {
            case: row.case,
            name: row.name.clone(),
            subject: format!("{} (created again)", row.subject),
        };
        create_template(&router, &recreated).await;
        assert_eq!(
            read_subject(&router, &row.name).await,
            Ok(Some(recreated.subject.clone())),
            "case {}: a create after a delete must record the new content",
            row.case
        );
        let updated_again = TemplateRow {
            case: row.case,
            name: row.name.clone(),
            subject: format!("{} (updated again)", row.subject),
        };
        let (status, body) = template_call(&router, "UpdateTemplate", &updated_again).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 updating the re-created template, got {status} with {body}",
            row.case
        );
        assert_eq!(
            read_subject(&router, &row.name).await,
            Ok(Some(updated_again.subject.clone())),
            "case {}: the update must replace the re-created template's content",
            row.case
        );
    }
}
