// SPDX-License-Identifier: CC0-1.0

//! `DeleteTemplate`, end to end over the wire, against the readers that prove
//! what it removed.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind
//! it. What is asserted is the RULE the operation follows over a SET of inputs
//! rather than one recorded request:
//!
//! * a delete removes the template recorded under the name the request names,
//!   whatever that name and whatever parts the template carried, so `GetTemplate`
//!   then refuses the name and `ListTemplates` omits it, while every other
//!   template is left exactly as it was;
//! * the operation models no not-found error, so deleting a name the account
//!   holds no template under answers 200 and changes nothing — and a repeated
//!   delete of the same name is that same success;
//! * a deleted name is free again: creating it a second time is accepted and the
//!   reader reports the NEW parts.

use crate::harness::{call, element, elements, emulator, emulator_over, field, xml_text, Field};
use service_com_amazonaws_ses_simpleemailservice::state::service_state;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// One template as a request carries it. A `None` part is a member the request
/// does not send at all, which is a different request from one sending an empty
/// string.
#[derive(Debug, Clone)]
struct TemplateRow {
    case: &'static str,
    name: String,
    subject: Option<String>,
    text: Option<String>,
    html: Option<String>,
}

/// The parts a reader reports for one template: the name, and each part it
/// carries (`None` when the member is absent from the response).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedTemplate {
    name: Option<String>,
    subject: Option<String>,
    text: Option<String>,
    html: Option<String>,
}

impl TemplateRow {
    /// A row naming every part, so a case only has to say what makes it
    /// different.
    fn new(case: &'static str, name: &str, subject: &str, text: &str, html: &str) -> Self {
        Self {
            case,
            name: name.to_string(),
            subject: Some(subject.to_string()),
            text: Some(text.to_string()),
            html: Some(html.to_string()),
        }
    }

    /// What a reader must report for this row while it is stored.
    fn as_reported(&self) -> ReportedTemplate {
        ReportedTemplate {
            name: Some(self.name.clone()),
            subject: self.subject.clone(),
            text: self.text.clone(),
            html: self.html.clone(),
        }
    }
}

/// `CreateTemplate`, asserting it was accepted. Only the parts the row names are
/// sent.
async fn create_template(router: &MultiServiceRouter, row: &TemplateRow) {
    let mut params: Vec<Field> = vec![field("Template.TemplateName", row.name.clone())];
    for (member, value) in [
        ("Template.SubjectPart", &row.subject),
        ("Template.TextPart", &row.text),
        ("Template.HtmlPart", &row.html),
    ] {
        if let Some(value) = value {
            params.push(field(member, value.clone()));
        }
    }
    let (status, body) = call(router, "CreateTemplate", &params).await;
    assert_eq!(
        status, 200,
        "case {}: expected 200 from CreateTemplate, got {status} with {body}",
        row.case
    );
}

/// One `DeleteTemplate` call: `(status, body)`.
async fn delete_template_raw(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "DeleteTemplate", &[field("TemplateName", name)]).await
}

/// `DeleteTemplate`, asserting the success this operation always answers with:
/// HTTP 200 carrying its own awsQuery envelope.
async fn delete_template(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = delete_template_raw(router, name).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from DeleteTemplate({name:?}), got {status} with {body}"
    );
    assert!(
        body.contains("<DeleteTemplateResponse"),
        "case {case}: expected this operation's awsQuery envelope, got {body}"
    );
}

/// `GetTemplate`, as the parts it reports. Asserts the call succeeded and
/// carried a `Template`.
async fn get_template(router: &MultiServiceRouter, case: &str, name: &str) -> ReportedTemplate {
    let (status, body) = call(router, "GetTemplate", &[field("TemplateName", name)]).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from GetTemplate({name:?}), got {status} with {body}"
    );
    let template = element(&body, "Template").unwrap_or_else(|| {
        panic!("case {case}: GetTemplate({name:?}) must report a Template, got {body}")
    });
    let part = |tag: &str| element(&template, tag).map(|text| xml_text(&text));
    ReportedTemplate {
        name: part("TemplateName"),
        subject: part("SubjectPart"),
        text: part("TextPart"),
        html: part("HtmlPart"),
    }
}

/// Asserts `GetTemplate` refuses `name` with the modelled not-found error, which
/// is what "the template is gone" looks like to a reader.
async fn assert_template_absent(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = call(router, "GetTemplate", &[field("TemplateName", name)]).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400 from GetTemplate({name:?}) after the delete, \
         got {status} with {body}"
    );
    assert_eq!(
        element(&body, "Code").as_deref(),
        Some("TemplateDoesNotExist"),
        "case {case}: expected the modelled TemplateDoesNotExist for {name:?}, got {body}"
    );
}

/// Every name `ListTemplates` reports, over as many pages as it takes.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let mut found = Vec::new();
    let mut token: Option<String> = None;
    // Bounded above every page count this suite can ask for, so a listing that
    // never terminates fails here instead of looping forever.
    for _ in 0..1_000 {
        let mut params: Vec<Field> = vec![field("MaxItems", "100")];
        if let Some(token) = &token {
            params.push(field("NextToken", token.clone()));
        }
        let (status, body) = call(router, "ListTemplates", &params).await;
        assert_eq!(
            status, 200,
            "ListTemplates must answer 200, got {status} with {body}"
        );
        let page = element(&body, "TemplatesMetadata")
            .map(|list| elements(&list, "member"))
            .unwrap_or_default();
        for entry in &page {
            found.push(
                element(entry, "Name")
                    .unwrap_or_else(|| panic!("a listed entry carries Name, got {entry}")),
            );
        }
        match element(&body, "NextToken") {
            None => return found,
            Some(next) => token = Some(next),
        }
    }
    panic!("ListTemplates never ran out of pages");
}

/// The classes a stored template can be drawn from: each row differs in the kind
/// of text its parts hold, in which parts it carries at all, or in the shape of
/// its name.
fn stored_rows() -> Vec<TemplateRow> {
    let long_body = "x".repeat(4_096);
    vec![
        TemplateRow::new(
            "every part, plain text",
            "plain",
            "Your receipt",
            "Thanks for your order.",
            "<h1>Thanks for your order.</h1>",
        ),
        TemplateRow {
            case: "the text part only",
            name: "text-only".to_string(),
            subject: Some("Text only".to_string()),
            text: Some("No HTML here.".to_string()),
            html: None,
        },
        TemplateRow {
            case: "the HTML part only",
            name: "html_only".to_string(),
            subject: Some("HTML only".to_string()),
            text: None,
            html: Some("<p>No text part.</p>".to_string()),
        },
        TemplateRow {
            case: "the subject line alone",
            name: "SubjectOnly9".to_string(),
            subject: Some("Subject only".to_string()),
            text: None,
            html: None,
        },
        TemplateRow::new(
            "non-ASCII text in every part",
            "unicode-parts",
            "Grüße, 你好 — приве́т 🎉",
            "Übergrößenträger… 日本語のテキスト 🚀",
            "<p>Ünïcödé 😀 — 中文</p>",
        ),
        TemplateRow::new(
            "placeholder markup a send would render",
            "with-placeholders",
            "Hello {{name}}",
            "{{#if member}}Welcome back, {{name}}.{{else}}Welcome, {{name}}.{{/if}}",
            "<p>{{greeting}}</p>",
        ),
        TemplateRow::new(
            "XML metacharacters, which travel escaped",
            "metacharacters",
            "5 < 6 & \"quoted\" 'single'",
            "a > b && c < d",
            "<p class=\"a&b\">x &amp; y</p>",
        ),
        TemplateRow::new(
            "bodies of a few kilobytes",
            "long-bodies",
            "A long one",
            &long_body,
            &format!("<p>{long_body}</p>"),
        ),
        TemplateRow::new(
            "a name of digits",
            "0123456789",
            "Digits",
            "Digits.",
            "<p>Digits.</p>",
        ),
        TemplateRow::new(
            "a name with dashes and underscores",
            "a-name_with-both",
            "Mixed punctuation",
            "Mixed.",
            "<p>Mixed.</p>",
        ),
        TemplateRow::new(
            "a name differing from another only by case",
            "PLAIN",
            "Upper case name",
            "Upper.",
            "<p>Upper.</p>",
        ),
        TemplateRow::new(
            "a 64-character name",
            &"n".repeat(64),
            "A long name",
            "Long name.",
            "<p>Long name.</p>",
        ),
    ]
}

#[tokio::test]
async fn deleting_a_template_removes_exactly_that_one_from_every_reader() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let store = service_state(&*engine).expect("this service has a store on the shared engine");

    let rows = stored_rows();
    for row in &rows {
        create_template(&router, row).await;
    }

    // One row at a time, so each delete is asserted against the templates that
    // are still there: the name goes, nothing else does.
    for (index, row) in rows.iter().enumerate() {
        delete_template(&router, row.case, &row.name).await;
        assert_template_absent(&router, row.case, &row.name).await;

        let mut expected: Vec<String> = rows[index + 1..]
            .iter()
            .map(|other| other.name.clone())
            .collect();
        expected.sort();
        let names = listed_names(&router).await;
        assert_eq!(
            names, expected,
            "case {}: the listing must hold only the templates not yet deleted, got {names:?}",
            row.case
        );
        assert!(
            !store.lock().templates.contains_key(&row.name),
            "case {}: the store must no longer hold {:?}",
            row.case,
            row.name
        );

        // The survivors still report the parts their create sent, part for part.
        for other in &rows[index + 1..] {
            let reported = get_template(&router, other.case, &other.name).await;
            assert_eq!(
                reported,
                other.as_reported(),
                "case {}: deleting {:?} must leave {:?} exactly as it was",
                other.case,
                row.name,
                other.name
            );
        }
    }

    let names = listed_names(&router).await;
    assert!(
        names.is_empty(),
        "case the whole table: every template was deleted, got {names:?}"
    );
}

#[tokio::test]
async fn deleting_a_name_the_account_holds_no_template_under_succeeds_and_changes_nothing() {
    let router = emulator();
    let stored = TemplateRow::new(
        "the one stored template",
        "still-here",
        "The stored subject",
        "The stored text.",
        "<p>The stored text.</p>",
    );
    create_template(&router, &stored).await;

    // Each case is a name no create recorded: names near the stored one, names
    // of other shapes, and the stored name deleted twice over.
    let repeated = stored.name.clone();
    let cases: Vec<(&str, String)> = vec![
        ("a name nothing ever created", "never-created".to_string()),
        (
            "a name differing from the stored one only by case",
            stored.name.to_uppercase(),
        ),
        (
            "a prefix of the stored name",
            stored.name[..5].to_string(),
        ),
        (
            "the stored name with trailing whitespace",
            format!("{} ", stored.name),
        ),
        ("a non-ASCII name", "шаблон-🎉".to_string()),
        ("a name of digits", "0123456789".to_string()),
        ("an empty name", String::new()),
        ("a 64-character name", "z".repeat(64)),
    ];
    for (case, name) in &cases {
        delete_template(&router, case, name).await;
        let reported = get_template(&router, case, &stored.name).await;
        assert_eq!(
            reported,
            stored.as_reported(),
            "case {case}: deleting {name:?} must leave the stored template alone"
        );
        let names = listed_names(&router).await;
        assert_eq!(
            names,
            vec![stored.name.clone()],
            "case {case}: the listing must be unchanged, got {names:?}"
        );
    }

    // Deleting the same name twice is the same success: the second call has
    // nothing to remove and says so the same way.
    delete_template(&router, "the first delete", &repeated).await;
    delete_template(&router, "the same name again", &repeated).await;
    assert_template_absent(&router, "the same name again", &repeated).await;
    let names = listed_names(&router).await;
    assert!(
        names.is_empty(),
        "case the repeated delete: the account must hold no template, got {names:?}"
    );
}

#[tokio::test]
async fn a_deleted_name_is_free_for_a_new_template() {
    let router = emulator();
    // Each row reuses the name the row before it deleted, with different parts
    // every time: what the reader reports afterwards is the LATEST create.
    let name = "reused-name";
    let rows = vec![
        TemplateRow::new(
            "the first create",
            name,
            "First subject",
            "First text.",
            "<p>First text.</p>",
        ),
        TemplateRow {
            case: "fewer parts than before",
            name: name.to_string(),
            subject: Some("Second subject".to_string()),
            text: None,
            html: None,
        },
        TemplateRow::new(
            "non-ASCII parts",
            name,
            "Dritter Betreff — 件名 🎉",
            "Dritter Text… 本文 🚀",
            "<p>Drittes HTML — 中文</p>",
        ),
    ];
    for row in &rows {
        create_template(&router, row).await;
        let reported = get_template(&router, row.case, &row.name).await;
        assert_eq!(
            reported,
            row.as_reported(),
            "case {}: the reader must report the parts this create sent",
            row.case
        );
        delete_template(&router, row.case, &row.name).await;
        assert_template_absent(&router, row.case, &row.name).await;
    }
    let names = listed_names(&router).await;
    assert!(
        names.is_empty(),
        "case after {} create-delete rounds: the account must hold no template, got {names:?}",
        rows.len()
    );
}
