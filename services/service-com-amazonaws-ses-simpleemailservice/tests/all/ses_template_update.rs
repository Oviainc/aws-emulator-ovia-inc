// SPDX-License-Identifier: CC0-1.0

//! `UpdateTemplate`, and the readers that prove what it recorded, end to end over
//! the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each call
//! is a real awsQuery request against the service: form-urlencoded body in, XML
//! out, one shared engine behind it. What is asserted is the RULE the operation
//! follows over a SET of inputs rather than one recorded request:
//!
//! * an update REPLACES the content of a template the account already holds with
//!   the parts the request carries — whatever those parts are, and leaving a part
//!   the request does not send absent — while the creation instant the listing
//!   reports stays the instant of the create;
//! * a name the account holds no template under is refused with the modelled
//!   `TemplateDoesNotExist` and records nothing, whatever parts the request
//!   carries;
//! * an update that would leave the template with no subject line is refused with
//!   HTTP 400 `InvalidParameterValue` and leaves the stored template alone.

use crate::harness::{call, element, elements, emulator, field, xml_text, Field};
use smithy_server_runtime::MultiServiceRouter;

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

    /// What a reader must report for this row once it is stored.
    fn as_reported(&self) -> ReportedTemplate {
        ReportedTemplate {
            name: Some(self.name.clone()),
            subject: self.subject.clone(),
            text: self.text.clone(),
            html: self.html.clone(),
        }
    }
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope, with
/// the entities the XML codec escapes read back.
fn error_envelope(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code"),
        element(body, "Message").map(|text| xml_text(&text)),
    )
}

/// The `Template.*` form fields for one row. Only the parts the row names are
/// sent.
fn template_fields(row: &TemplateRow) -> Vec<Field> {
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
    params
}

/// `CreateTemplate`, asserting it was accepted — the precondition an update
/// needs, not what this suite is about.
async fn create_template(router: &MultiServiceRouter, row: &TemplateRow) {
    let (status, body) = call(router, "CreateTemplate", &template_fields(row)).await;
    assert_eq!(
        status, 200,
        "case {}: expected 200 from CreateTemplate, got {status} with {body}",
        row.case
    );
}

/// One `UpdateTemplate` call: `(status, body)`.
async fn update_template_raw(router: &MultiServiceRouter, row: &TemplateRow) -> (u16, String) {
    call(router, "UpdateTemplate", &template_fields(row)).await
}

/// `UpdateTemplate`, asserting it was accepted.
async fn update_template(router: &MultiServiceRouter, row: &TemplateRow) {
    let (status, body) = update_template_raw(router, row).await;
    assert_eq!(
        status, 200,
        "case {}: expected 200 from UpdateTemplate, got {status} with {body}",
        row.case
    );
    assert!(
        body.contains("<UpdateTemplateResponse"),
        "case {}: expected this operation's awsQuery envelope, got {body}",
        row.case
    );
}

/// One `GetTemplate` call: `(status, body)`.
async fn get_template_raw(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "GetTemplate", &[field("TemplateName", name)]).await
}

/// `GetTemplate`, as the parts it reports. Asserts the call succeeded and carried
/// a `Template`.
async fn get_template(router: &MultiServiceRouter, case: &str, name: &str) -> ReportedTemplate {
    let (status, body) = get_template_raw(router, name).await;
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

/// Every `(name, creation second)` the listing reports, over as many pages as it
/// takes.
async fn listed(router: &MultiServiceRouter) -> Vec<(String, i64)> {
    let mut found = Vec::new();
    let mut token: Option<String> = None;
    // Bounded above every page count this suite can ask for, so a listing that
    // never terminates fails here instead of looping forever.
    for _ in 0..100 {
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
            let name = element(entry, "Name")
                .unwrap_or_else(|| panic!("a listed entry carries Name, got {entry}"));
            let stamp = element(entry, "CreatedTimestamp")
                .unwrap_or_else(|| panic!("a listed entry carries CreatedTimestamp, got {entry}"));
            let parsed = ::aws_smithy_types::DateTime::from_str(
                &stamp,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .unwrap_or_else(|error| {
                panic!("case {name:?}: CreatedTimestamp {stamp:?} must be a date-time: {error}")
            });
            found.push((name, parsed.secs()));
        }
        match element(&body, "NextToken") {
            None => return found,
            Some(next) => token = Some(next),
        }
    }
    panic!("ListTemplates never ran out of pages");
}

/// The template the content cases are applied to: one stored template per case,
/// so each update is independent of the others.
fn starting_point(case: &'static str, name: &str) -> TemplateRow {
    TemplateRow::new(
        case,
        name,
        "The subject the create stored",
        "The text the create stored.",
        "<p>The text the create stored.</p>",
    )
}

/// `(the update a case sends)` for each content class an update can carry: the
/// rows differ in the kind of text the parts hold, and in which parts are sent at
/// all.
fn update_rows() -> Vec<TemplateRow> {
    let long_body = "y".repeat(4_096);
    vec![
        TemplateRow::new(
            "every part replaced with plain text",
            "plain",
            "A new subject",
            "New text.",
            "<h1>New text.</h1>",
        ),
        // "either an HTML part or a text-only part" (`Template`): an update that
        // sends one body, the other, or neither.
        TemplateRow {
            case: "the text part only, so the HTML part is dropped",
            name: "text-only".to_string(),
            subject: Some("Text only now".to_string()),
            text: Some("No HTML any more.".to_string()),
            html: None,
        },
        TemplateRow {
            case: "the HTML part only, so the text part is dropped",
            name: "html_only".to_string(),
            subject: Some("HTML only now".to_string()),
            text: None,
            html: Some("<p>No text part any more.</p>".to_string()),
        },
        TemplateRow {
            case: "the subject line alone, so both bodies are dropped",
            name: "SubjectOnly9".to_string(),
            subject: Some("Subject only now".to_string()),
            text: None,
            html: None,
        },
        TemplateRow::new(
            "placeholder markup, which a send renders and an update stores as sent",
            "with-placeholders",
            "Hello again {{name}}",
            "{{#if member}}Still a member, {{name}}.{{else}}Welcome, {{name}}.{{/if}}",
            "<p>{{greeting}}</p>",
        ),
        TemplateRow::new(
            "non-ASCII text in every part",
            "unicode-parts",
            "Grüße, 你好 — приве́т 🎉",
            "Übergrößenträger… 日本語のテキスト 🚀",
            "<p>Ünïcödé 😀 — 中文</p>",
        ),
        TemplateRow::new(
            "XML metacharacters, which travel escaped and must come back unchanged",
            "metacharacters",
            "7 < 8 & \"quoted\" 'single'",
            "a > b && c < d",
            "<p class=\"a&b\">x &amp; y</p>",
        ),
        TemplateRow::new(
            "whitespace-only bodies, which are content and not absence",
            "whitespace-bodies",
            " ",
            "\n\t ",
            " ",
        ),
        TemplateRow::new(
            "bodies of a few kilobytes",
            "long-bodies",
            "A long one",
            &long_body,
            &format!("<p>{long_body}</p>"),
        ),
        TemplateRow::new(
            "an update that changes nothing but the subject",
            "subject-only-change",
            "Only the subject moved",
            "The text the create stored.",
            "<p>The text the create stored.</p>",
        ),
        // The name itself is caller input: the classes a template name can be
        // drawn from, including one that differs from another row only by case.
        TemplateRow::new(
            "a name of digits",
            "0123456789",
            "Digits again",
            "Digits.",
            "<p>Digits.</p>",
        ),
        TemplateRow::new(
            "a name with dashes and underscores",
            "a-name_with-both",
            "Mixed punctuation again",
            "Mixed.",
            "<p>Mixed.</p>",
        ),
        TemplateRow::new(
            "a name differing from another only by case",
            "PLAIN",
            "Upper case name again",
            "Upper.",
            "<p>Upper.</p>",
        ),
        TemplateRow::new(
            "a 64-character name",
            &"n".repeat(64),
            "A long name again",
            "Long name.",
            "<p>Long name.</p>",
        ),
    ]
}

#[tokio::test]
async fn an_update_replaces_the_stored_content_with_the_parts_the_request_carries() {
    let router = emulator();
    let rows = update_rows();

    // One stored template per case, created BEFORE any update, so the creation
    // instants an update must leave alone are recorded first.
    for row in &rows {
        create_template(&router, &starting_point(row.case, &row.name)).await;
    }
    let before = listed(&router).await;

    for row in &rows {
        update_template(&router, row).await;
        let reported = get_template(&router, row.case, &row.name).await;
        assert_eq!(
            reported,
            row.as_reported(),
            "case {}: the reader must report the parts the update sent, and only those",
            row.case
        );
    }

    // An update replaces content; it adds nothing to the account and renames
    // nothing, and the creation instant belongs to the create.
    let after = listed(&router).await;
    assert_eq!(
        after, before,
        "case the whole table: after {} updates the account must hold the same templates with the \
         same creation instants, got {after:?}",
        rows.len()
    );

    // Updating is idempotent in its result: sending the same content again
    // leaves the same parts, and so does reading twice.
    for row in &rows {
        update_template(&router, row).await;
        let reread = get_template(&router, row.case, &row.name).await;
        assert_eq!(
            reread,
            row.as_reported(),
            "case {}: a repeated update must leave the same parts",
            row.case
        );
    }
    let after_repeats = listed(&router).await;
    assert_eq!(
        after_repeats, before,
        "case after repeating every update: the account must be unchanged, got {after_repeats:?}"
    );
}

#[tokio::test]
async fn a_name_the_account_holds_no_template_under_is_refused_and_records_nothing() {
    let router = emulator();
    let stored = starting_point("the template that does exist", "the-stored-one");
    create_template(&router, &stored).await;
    let before = listed(&router).await;

    // Each row names a template the account does not hold, differing in the kind
    // of name it is and in the parts it would have written: the refusal is about
    // the absence and nothing else.
    let rows: Vec<TemplateRow> = vec![
        TemplateRow::new(
            "a name nothing was ever created under",
            "never-created",
            "A subject",
            "Some text.",
            "<p>Some text.</p>",
        ),
        TemplateRow::new(
            "the stored name in a different case",
            "The-Stored-One",
            "A subject",
            "Some text.",
            "<p>Some text.</p>",
        ),
        TemplateRow::new(
            "the stored name with a suffix",
            "the-stored-one-2",
            "A subject",
            "Some text.",
            "<p>Some text.</p>",
        ),
        TemplateRow {
            case: "an absent name, with only the subject sent",
            name: "absent-subject-only".to_string(),
            subject: Some("A subject".to_string()),
            text: None,
            html: None,
        },
        TemplateRow::new(
            "an absent name of digits",
            "987654321",
            "A subject",
            "Some text.",
            "<p>Some text.</p>",
        ),
        TemplateRow::new(
            "an absent name carrying non-ASCII text",
            "absent-ünïcödé-名前",
            "A subject",
            "Some text.",
            "<p>Some text.</p>",
        ),
        TemplateRow::new(
            "an absent 64-character name",
            &"z".repeat(64),
            "A subject",
            "Some text.",
            "<p>Some text.</p>",
        ),
    ];

    for row in &rows {
        let (status, body) = update_template_raw(&router, row).await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400 for a template the account does not hold, got {status} \
             with {body}",
            row.case
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("TemplateDoesNotExist"),
            "case {}: expected the modelled TemplateDoesNotExist, got {body}",
            row.case
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Template {} does not exist.", row.name).as_str()),
            "case {}: expected the absent-template message naming {:?}, got {body}",
            row.case,
            row.name
        );
        assert_eq!(
            element(&body, "TemplateName").map(|text| xml_text(&text)).as_deref(),
            Some(row.name.as_str()),
            "case {}: the refusal must name the template, got {body}",
            row.case
        );

        // The refusal recorded nothing: the name is still one the account holds
        // no template under, and the reader says so too.
        let (read_status, read_body) = get_template_raw(&router, &row.name).await;
        assert_eq!(
            read_status, 400,
            "case {}: a refused update must record nothing, got {read_status} with {read_body}",
            row.case
        );
        assert_eq!(
            error_envelope(&read_body).0.as_deref(),
            Some("TemplateDoesNotExist"),
            "case {}: the reader must still report the template as absent, got {read_body}",
            row.case
        );
    }

    let after = listed(&router).await;
    assert_eq!(
        after, before,
        "case after {} refusals: the account must hold only the template it started with, got \
         {after:?}",
        rows.len()
    );

    // The template that does exist was never touched by any of those refusals.
    let reported = get_template(&router, "the template that does exist", &stored.name).await;
    assert_eq!(
        reported,
        stored.as_reported(),
        "case the stored template: a refused update of another name must leave it alone"
    );
}

#[tokio::test]
async fn an_update_that_names_no_subject_line_is_refused_and_leaves_the_template_alone() {
    let router = emulator();
    let stored = starting_point("the stored template", "has-a-subject");
    create_template(&router, &stored).await;
    let before = listed(&router).await;

    // Each row omits the subject line, or sends it empty, while differing in the
    // bodies it carries: the refusal is about the subject and nothing else.
    let rows: Vec<TemplateRow> = vec![
        TemplateRow {
            case: "no subject member at all",
            name: stored.name.clone(),
            subject: None,
            text: Some("A body with no subject.".to_string()),
            html: Some("<p>A body with no subject.</p>".to_string()),
        },
        TemplateRow {
            case: "an empty subject",
            name: stored.name.clone(),
            subject: Some(String::new()),
            text: Some("A body with an empty subject.".to_string()),
            html: None,
        },
        TemplateRow {
            case: "an empty subject and no bodies",
            name: stored.name.clone(),
            subject: Some(String::new()),
            text: None,
            html: None,
        },
        TemplateRow {
            case: "no subject and no bodies",
            name: stored.name.clone(),
            subject: None,
            text: None,
            html: None,
        },
    ];

    for row in &rows {
        let (status, body) = update_template_raw(&router, row).await;
        assert_eq!(
            status, 400,
            "case {}: expected HTTP 400 for a template with no subject line, got {status} with \
             {body}",
            row.case
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidParameterValue"),
            "case {}: expected InvalidParameterValue, got {body}",
            row.case
        );
        assert_eq!(
            message.as_deref(),
            Some("The subject must be specified."),
            "case {}: expected the missing-subject message, got {body}",
            row.case
        );

        // The refusal wrote nothing: the stored template still reports the parts
        // the create sent.
        let reported = get_template(&router, row.case, &stored.name).await;
        assert_eq!(
            reported,
            stored.as_reported(),
            "case {}: a refused update must leave the stored template alone",
            row.case
        );
    }

    let after = listed(&router).await;
    assert_eq!(
        after, before,
        "case after {} refusals: the account must be unchanged, got {after:?}",
        rows.len()
    );
}
