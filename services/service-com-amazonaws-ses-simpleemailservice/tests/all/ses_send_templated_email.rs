// SPDX-License-Identifier: CC0-1.0

//! `SendTemplatedEmail` end to end over the wire, and the readers that prove what
//! it recorded.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each call
//! is a real awsQuery request against the service: form-urlencoded body in, XML
//! out, one shared engine behind it. What is asserted is the RULE the operation
//! follows over a SET of inputs rather than one recorded request:
//!
//! * a send composes its message from one of the account's stored email templates
//!   and is ACCEPTED with the message id the service logged, whatever recipient
//!   set, template parts, replacement values, tags or configuration set the
//!   request carries — and every recipient of it is counted against the account's
//!   24-hour sending quota, which is what reads the send back;
//! * the checks a send passes run in ONE order, each refusing with its own
//!   modelled error: the recipient set the request carries, then the account's
//!   sending state, then the configuration set it names, then the sending
//!   identity, and only then the template — so a call from an identity this
//!   account has not verified is refused for the identity even when the template
//!   it named does not exist either;
//! * a refused send records nothing: the send log the quota is derived from is
//!   untouched, and the template store is left exactly as it was.
//!
//! The rows differ along what the rule generalises over: which recipient fields
//! carry the destination and how many addresses they hold (up to the documented
//! ceiling), which parts the stored template carries, what the replacement values
//! are (none, flat, nested), whether the send names a configuration set or tags,
//! and which gate a row is built to fail.

use crate::harness::{
    call, element, emulator_and_engine, error_envelope, field, settle_verifications, xml_text,
    Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::{
    DEFAULT_REGION, MAX_RECIPIENTS, SANDBOX_MAX_24_HOUR_SEND,
};
use smithy_server_runtime::MultiServiceRouter;

/// The identity every accepted send below is sent from, verified through
/// `VerifyEmailIdentity`.
const SENDER: &str = "sender@example.com";

/// An identity whose verification is only REQUESTED in the suite that needs one,
/// so the identity gate has something to refuse.
const PENDING_SENDER: &str = "pending@example.com";

/// The domain every recipient address below is in, verified so that a recipient
/// is never the reason a send is refused.
const RECIPIENT_DOMAIN: &str = "example.net";

/// The configuration set a send may name, created with sending enabled.
const LIVE_SET: &str = "live-set";

/// A configuration set the account holds with its sending PAUSED.
const PAUSED_SET: &str = "paused-set";

/// One stored email template, as `CreateTemplate` carries it. A `None` part is a
/// member the request does not send at all.
struct Template {
    /// The name the template is stored under.
    name: &'static str,
    /// `Template$SubjectPart`, which `CreateTemplate` requires.
    subject: &'static str,
    /// `Template$TextPart`, when this template carries one.
    text: Option<&'static str>,
    /// `Template$HtmlPart`, when this template carries one.
    html: Option<&'static str>,
}

/// The templates this suite stores: the parts a template can be built from, each
/// carrying the `{{placeholder}}` markup a templated send supplies values for.
fn templates() -> Vec<Template> {
    vec![
        Template {
            name: "every-part",
            subject: "Hello {{name}}",
            text: Some("Dear {{name}}, your order {{order}} shipped."),
            html: Some("<p>Dear {{name}}</p>"),
        },
        Template {
            name: "text-only",
            subject: "Receipt for {{order}}",
            text: Some("Thanks, {{name}}."),
            html: None,
        },
        Template {
            name: "html-only",
            subject: "Statement",
            text: None,
            html: Some("<h1>{{name}}</h1>"),
        },
        Template {
            name: "subject-only",
            subject: "Reminder",
            text: None,
            html: None,
        },
    ]
}

/// One `SendTemplatedEmail` request.
struct Send {
    /// What makes this row different from the others.
    case: String,
    /// The `Source` the message is sent from.
    source: String,
    /// `Destination.ToAddresses`.
    to: Vec<String>,
    /// `Destination.CcAddresses`.
    cc: Vec<String>,
    /// `Destination.BccAddresses`.
    bcc: Vec<String>,
    /// The `Template` the send names.
    template: String,
    /// `TemplateData`, the replacement values for the template's placeholders.
    template_data: String,
    /// `ConfigurationSetName`, when the send names one.
    configuration_set: Option<String>,
    /// `Tags`, in request order.
    tags: Vec<(String, String)>,
}

impl Send {
    /// A send from [`SENDER`] of `template` to `to`, which a row then varies.
    fn new(case: &str, template: &str, to: &[&str]) -> Self {
        Self {
            case: case.to_string(),
            source: SENDER.to_string(),
            to: to.iter().map(|address| address.to_string()).collect(),
            cc: Vec::new(),
            bcc: Vec::new(),
            template: template.to_string(),
            template_data: "{}".to_string(),
            configuration_set: None,
            tags: Vec::new(),
        }
    }

    /// Every recipient this send addresses, across the three recipient fields —
    /// the count that is charged against the sending quota.
    fn recipient_count(&self) -> usize {
        self.to.len() + self.cc.len() + self.bcc.len()
    }

    /// The form fields that carry this send on the wire.
    fn fields(&self) -> Vec<Field> {
        let mut params = vec![
            field("Source", self.source.clone()),
            field("Template", self.template.clone()),
            field("TemplateData", self.template_data.clone()),
        ];
        for (member, addresses) in [
            ("ToAddresses", &self.to),
            ("CcAddresses", &self.cc),
            ("BccAddresses", &self.bcc),
        ] {
            for (index, address) in addresses.iter().enumerate() {
                params.push(field(
                    format!("Destination.{member}.member.{}", index + 1),
                    address.clone(),
                ));
            }
        }
        if let Some(name) = &self.configuration_set {
            params.push(field("ConfigurationSetName", name.clone()));
        }
        for (index, (name, value)) in self.tags.iter().enumerate() {
            params.push(field(format!("Tags.member.{}.Name", index + 1), name.clone()));
            params.push(field(
                format!("Tags.member.{}.Value", index + 1),
                value.clone(),
            ));
        }
        params
    }
}

/// `recipient-<n>@example.net`, so a row can address as many recipients as it
/// needs in a domain this account has verified.
fn recipients(count: usize) -> Vec<String> {
    (1..=count)
        .map(|index| format!("recipient-{index}@{RECIPIENT_DOMAIN}"))
        .collect()
}

/// An emulator holding the identities, templates and configuration sets every
/// case below sends against, with the sending identity's verification carried
/// through.
async fn emulator_ready() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_and_engine();
    let setup: Vec<(&str, Vec<Field>)> = vec![
        ("VerifyEmailIdentity", vec![field("EmailAddress", SENDER)]),
        (
            "VerifyDomainIdentity",
            vec![field("Domain", RECIPIENT_DOMAIN)],
        ),
        (
            "CreateConfigurationSet",
            vec![field("ConfigurationSet.Name", LIVE_SET)],
        ),
        (
            "CreateConfigurationSet",
            vec![field("ConfigurationSet.Name", PAUSED_SET)],
        ),
        (
            "UpdateConfigurationSetSendingEnabled",
            vec![
                field("ConfigurationSetName", PAUSED_SET),
                field("Enabled", "false"),
            ],
        ),
    ];
    for (action, params) in setup {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case setup {action}: expected 200, got {status} with {body}"
        );
    }
    for template in templates() {
        let mut params = vec![
            field("Template.TemplateName", template.name),
            field("Template.SubjectPart", template.subject),
        ];
        if let Some(text) = template.text {
            params.push(field("Template.TextPart", text));
        }
        if let Some(html) = template.html {
            params.push(field("Template.HtmlPart", html));
        }
        let (status, body) = call(&router, "CreateTemplate", &params).await;
        assert_eq!(
            status, 200,
            "case setup CreateTemplate {}: expected 200, got {status} with {body}",
            template.name
        );
    }
    // Asking for a verification only STARTS it, so the store is settled past the
    // span the clock owes those records before any send goes out.
    settle_verifications(&engine);
    (router, engine)
}

/// `GetSendQuota`'s `SentLast24Hours`, which is DERIVED from the send log: it is
/// how a case reads back whether a send reached it.
async fn sent_last_24_hours(router: &MultiServiceRouter) -> f64 {
    let (status, body) = call(router, "GetSendQuota", &[]).await;
    assert_eq!(
        status, 200,
        "case GetSendQuota: expected 200, got {status} with {body}"
    );
    let text = element(&body, "SentLast24Hours")
        .unwrap_or_else(|| panic!("case GetSendQuota: expected SentLast24Hours, got {body}"));
    text.parse::<f64>()
        .unwrap_or_else(|_| panic!("case GetSendQuota: expected a number, got {text:?}"))
}

/// The `Template` element `GetTemplate` reports for `name`, as its raw XML, so a
/// case can assert a send left the stored template exactly as it was.
async fn stored_template(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status, body) = call(router, "GetTemplate", &[field("TemplateName", name)]).await;
    if status != 200 {
        return None;
    }
    element(&body, "Template")
}

/// The message a send from an identity this account has not verified is refused
/// with, as this service words it for the Region the harness signs for.
fn unverified_message(identity: &str) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in region \
         {}: {identity}",
        DEFAULT_REGION.to_uppercase()
    )
}

#[tokio::test]
async fn every_form_of_templated_send_is_accepted_and_counted() {
    let (router, _engine) = emulator_ready().await;

    // The rows span what the rule generalises over: which recipient fields carry
    // the destination and how many addresses they hold, which parts the template
    // carries, what the replacement values are, and whether the send names a
    // configuration set or tags.
    let mut rows = vec![
        Send::new("one To: recipient, a template with every part", "every-part", &["a@example.net"]),
        Send::new("a template with no HTML part", "text-only", &["b@example.net"]),
        Send::new("a template with no text part", "html-only", &["c@example.net"]),
        Send::new("a template with a subject line only", "subject-only", &["d@example.net"]),
    ];
    // Recipients in the CC: field alone, in the BCC: field alone, and across all
    // three — the ceiling is "across the To:, CC: and BCC: fields", so each field
    // is a recipient field like the others.
    let mut cc_only = Send::new("CC: recipients only", "every-part", &[]);
    cc_only.cc = recipients(2);
    rows.push(cc_only);
    let mut bcc_only = Send::new("BCC: recipients only", "every-part", &[]);
    bcc_only.bcc = recipients(1);
    rows.push(bcc_only);
    let mut mixed = Send::new("recipients across all three fields", "text-only", &["e@example.net"]);
    mixed.cc = vec!["f@example.net".to_string()];
    mixed.bcc = vec!["g@example.net".to_string()];
    rows.push(mixed);
    // The documented ceiling itself: "The Destination parameter can include up to
    // 50 recipients, across the To:, CC: and BCC: fields."
    let mut at_the_ceiling = Send::new("the documented recipient ceiling", "every-part", &[]);
    at_the_ceiling.to = recipients(MAX_RECIPIENTS - 2);
    at_the_ceiling.cc = recipients(1);
    at_the_ceiling.bcc = vec!["last@example.net".to_string()];
    rows.push(at_the_ceiling);
    // The replacement values: none at all, flat pairs, and a nested document.
    for (case, template_data) in [
        ("no replacement values", "{}"),
        ("flat replacement values", r#"{"name":"Ada","order":"4711"}"#),
        (
            "a nested replacement document",
            r#"{"name":"Ada","order":{"id":"4711","items":["pen","ink"]}}"#,
        ),
    ] {
        let mut row = Send::new(case, "every-part", &["values@example.net"]);
        row.template_data = template_data.to_string();
        rows.push(row);
    }
    // A send that names a configuration set the account holds, and one that
    // carries message tags.
    let mut with_set = Send::new("a configuration set the account holds", "every-part", &["set@example.net"]);
    with_set.configuration_set = Some(LIVE_SET.to_string());
    rows.push(with_set);
    let mut with_tags = Send::new("message tags", "every-part", &["tags@example.net"]);
    with_tags.tags = vec![
        ("campaign".to_string(), "spring".to_string()),
        ("segment".to_string(), "new".to_string()),
    ];
    rows.push(with_tags);
    // A display-name source resolves to the same mailbox the account verified.
    let mut display_name = Send::new("a Source in display-name form", "every-part", &["display@example.net"]);
    display_name.source = format!("Sales Desk <{SENDER}>");
    rows.push(display_name);

    let mut counted = 0.0;
    let mut ids: Vec<String> = Vec::new();
    for row in &rows {
        let (status, body) = call(&router, "SendTemplatedEmail", &row.fields()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body}",
            row.case
        );
        assert!(
            body.contains("<SendTemplatedEmailResponse"),
            "case {:?}: expected this operation's awsQuery envelope, got {body}",
            row.case
        );
        let message_id = element(&body, "MessageId").unwrap_or_default();
        assert!(
            !message_id.is_empty(),
            "case {:?}: an accepted send reports the id it stored, got {body}",
            row.case
        );
        assert!(
            !ids.contains(&message_id),
            "case {:?}: expected an id no earlier send answered with, got {message_id:?} among \
             {ids:?}",
            row.case
        );
        ids.push(message_id);

        // Every recipient of the message counts once against the account's
        // 24-hour quota, which is what reads the send log back.
        counted += row.recipient_count() as f64;
        let sent = sent_last_24_hours(&router).await;
        assert_eq!(
            sent, counted,
            "case {:?}: expected {counted} counted recipients after the send, got {sent}",
            row.case
        );
    }

    // The sends composed from the stored templates and changed none of them.
    for template in templates() {
        let reported = stored_template(&router, template.name).await;
        let reported = reported.unwrap_or_else(|| {
            panic!(
                "case {}: expected the template to still be readable after the sends",
                template.name
            )
        });
        // The parts arrive XML-escaped, so the escaping is read back before
        // comparing: an HTML part carries `<` and `>` of its own.
        for (member, stored) in [
            ("SubjectPart", Some(template.subject)),
            ("TextPart", template.text),
            ("HtmlPart", template.html),
        ] {
            let reported_part = element(&reported, member).map(|text| xml_text(&text));
            assert_eq!(
                reported_part.as_deref(),
                stored,
                "case {} {member}: expected the stored part to survive the sends, got {reported}",
                template.name
            );
        }
    }
}

#[tokio::test]
async fn the_checks_a_templated_send_passes_run_in_one_order() {
    let (router, _engine) = emulator_ready().await;
    // One identity whose verification was only REQUESTED, so a row can fail the
    // identity gate without the account holding no identities at all.
    let (status, body) = call(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", PENDING_SENDER)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case setup VerifyEmailIdentity: expected 200, got {status} with {body}"
    );

    /// A row: the send, and the modelled code and message it is refused with.
    struct Refusal {
        send: Send,
        code: &'static str,
        message: String,
    }

    const MISSING: &str = "no-such-template";
    let mut rows: Vec<Refusal> = Vec::new();

    // The template is resolved LAST of the checks: a verified sender naming a
    // template the account does not hold is refused for the template.
    rows.push(Refusal {
        send: Send::new("a template the account does not hold", MISSING, &["a@example.net"]),
        code: "TemplateDoesNotExist",
        message: format!("Template {MISSING} does not exist."),
    });

    // ... and the same call from a sender this account has not verified is
    // refused for the IDENTITY, whichever template it named: the identity gate is
    // ahead of the template lookup.
    for (case, source) in [
        ("a sender whose verification was only requested", PENDING_SENDER),
        ("a sender nobody ever asked about", "stranger@example.com"),
    ] {
        for template in [MISSING, "every-part"] {
            let mut send = Send::new(
                &format!("{case}, naming the template {template}"),
                template,
                &["a@example.net"],
            );
            send.source = source.to_string();
            rows.push(Refusal {
                send,
                code: "MessageRejected",
                message: unverified_message(source),
            });
        }
    }

    // The configuration set the send names is read ahead of the identity and the
    // template both.
    let mut unknown_set = Send::new(
        "a configuration set the account does not hold, an unverified sender and a missing \
         template",
        MISSING,
        &["a@example.net"],
    );
    unknown_set.source = PENDING_SENDER.to_string();
    unknown_set.configuration_set = Some("no-such-set".to_string());
    rows.push(Refusal {
        send: unknown_set,
        code: "ConfigurationSetDoesNotExist",
        message: "Configuration set <no-such-set> does not exist.".to_string(),
    });
    let mut paused_set = Send::new(
        "a configuration set whose sending is paused, an unverified sender and a missing template",
        MISSING,
        &["a@example.net"],
    );
    paused_set.source = PENDING_SENDER.to_string();
    paused_set.configuration_set = Some(PAUSED_SET.to_string());
    rows.push(Refusal {
        send: paused_set,
        code: "ConfigurationSetSendingPausedException",
        message: format!("Email sending is disabled for configuration set <{PAUSED_SET}>."),
    });

    // The recipient set is read FIRST, before anything the account holds: a
    // destination past the ceiling, or carrying an address that is not an
    // address, is refused for the destination even when the set, the sender and
    // the template would all have failed too.
    // The recipient set is the FIRST thing the send reads. A request that omits
    // the `Destination` member altogether never reaches the handler at all: the
    // member is modelled required, so the boundary refuses it ahead of every gate
    // below, whichever set, sender and template the request named.
    let mut no_destination = Send::new("no Destination member at all", MISSING, &[]);
    no_destination.source = PENDING_SENDER.to_string();
    no_destination.configuration_set = Some("no-such-set".to_string());
    rows.push(Refusal {
        send: no_destination,
        code: "ValidationError",
        message: "1 validation error detected: Value at 'destination' failed to satisfy \
                  constraint: Member must not be null"
            .to_string(),
    });
    let mut past_the_ceiling = Send::new(
        "one recipient past the documented ceiling",
        MISSING,
        &["a@example.net"],
    );
    past_the_ceiling.source = PENDING_SENDER.to_string();
    past_the_ceiling.configuration_set = Some("no-such-set".to_string());
    past_the_ceiling.to = recipients(MAX_RECIPIENTS + 1);
    rows.push(Refusal {
        send: past_the_ceiling,
        code: "MessageRejected",
        message: format!(
            "Destination may include at most {MAX_RECIPIENTS} recipients across the ToAddresses, \
             CcAddresses and BccAddresses fields, got {}",
            MAX_RECIPIENTS + 1
        ),
    });
    for (case, invalid) in [
        ("a recipient with no domain", "nobody"),
        ("a recipient with no local part", "@example.net"),
        ("a recipient with a doubled dot", "first..last@example.net"),
    ] {
        let mut send = Send::new(case, MISSING, &["a@example.net"]);
        send.source = PENDING_SENDER.to_string();
        send.cc = vec![invalid.to_string()];
        rows.push(Refusal {
            send,
            code: "MessageRejected",
            message: format!(
                "Illegal address: {invalid:?} is not of the form \
                 UserName@[SubDomain.]Domain.TopLevelDomain"
            ),
        });
    }

    for row in &rows {
        let (status, body) = call(&router, "SendTemplatedEmail", &row.send.fields()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {body}",
            row.send.case
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (code.as_deref(), message.as_deref()),
            (Some(row.code), Some(row.message.as_str())),
            "case {:?}: expected {:?} / {:?}, got {code:?} / {message:?}",
            row.send.case,
            row.code,
            row.message
        );
    }

    // No refused send reached the send log the quota is derived from, and none of
    // them created the configuration set or the template it named.
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, 0.0,
        "case after the refusals: expected an untouched send log, got {sent}"
    );
    assert!(
        stored_template(&router, MISSING).await.is_none(),
        "case after the refusals: a refused send must not create the template it named"
    );
    let (status, body) = call(
        &router,
        "DescribeConfigurationSet",
        &[field("ConfigurationSetName", "no-such-set")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case after the refusals: a refused send must not create the set it named, got {status} \
         with {body}"
    );
}

#[tokio::test]
async fn a_templated_send_is_refused_while_the_account_is_paused() {
    let (router, _engine) = emulator_ready().await;
    // The account-wide pause is read before the configuration set, the identity
    // and the template: a row that would also have failed each of those is
    // refused for the account.
    let (status, body) = call(
        &router,
        "UpdateAccountSendingEnabled",
        &[field("Enabled", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateAccountSendingEnabled: expected 200, got {status} with {body}"
    );

    let mut paused = Send::new(
        "an unverified sender and a missing template while the account is paused",
        "no-such-template",
        &["a@example.net"],
    );
    paused.source = "stranger@example.com".to_string();
    paused.configuration_set = Some("no-such-set".to_string());
    let rows = [
        Send::new("a send that would otherwise be accepted", "every-part", &["a@example.net"]),
        paused,
    ];
    for row in &rows {
        let (status, body) = call(&router, "SendTemplatedEmail", &row.fields()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {body}",
            row.case
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (code.as_deref(), message.as_deref()),
            (
                Some("AccountSendingPausedException"),
                Some(
                    "Email sending is disabled for this account. Enable it with \
                     UpdateAccountSendingEnabled."
                )
            ),
            "case {:?}: expected the account pause, got {code:?} / {message:?}",
            row.case
        );
    }
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, 0.0,
        "case while paused: expected an untouched send log, got {sent}"
    );

    // Enabling sending again serves the same send, so the refusal was about the
    // account's state and nothing else.
    let (status, body) = call(
        &router,
        "UpdateAccountSendingEnabled",
        &[field("Enabled", "true")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateAccountSendingEnabled back on: expected 200, got {status} with {body}"
    );
    let (status, body) = call(&router, "SendTemplatedEmail", &rows[0].fields()).await;
    assert_eq!(
        status, 200,
        "case once sending is enabled again: expected 200, got {status} with {body}"
    );
}

#[tokio::test]
async fn every_recipient_of_a_templated_send_counts_against_the_daily_quota() {
    let (router, _engine) = emulator_ready().await;
    // The sandbox quota is the account's own stored limit, and each recipient of
    // an accepted send counts once against it, so the send that would take the
    // account past it is refused instead of accepted.
    let per_send = MAX_RECIPIENTS;
    let accepted_sends = (SANDBOX_MAX_24_HOUR_SEND as usize) / per_send;
    for round in 1..=accepted_sends {
        let mut send = Send::new("a send filling the quota", "every-part", &[]);
        send.to = recipients(per_send);
        let (status, body) = call(&router, "SendTemplatedEmail", &send.fields()).await;
        assert_eq!(
            status, 200,
            "case quota round {round}: expected 200, got {status} with {body}"
        );
    }
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, SANDBOX_MAX_24_HOUR_SEND,
        "case the filled quota: expected {SANDBOX_MAX_24_HOUR_SEND} counted recipients, got {sent}"
    );

    let past_the_quota = Send::new("one recipient past the quota", "every-part", &["a@example.net"]);
    let (status, body) = call(&router, "SendTemplatedEmail", &past_the_quota.fields()).await;
    assert_eq!(
        status, 400,
        "case {:?}: expected HTTP 400, got {status} with {body}",
        past_the_quota.case
    );
    let (code, message) = error_envelope(&body);
    let expected = format!(
        "Daily message quota exceeded: this account may send {SANDBOX_MAX_24_HOUR_SEND} messages \
         per 24 hours and has sent {SANDBOX_MAX_24_HOUR_SEND}"
    );
    assert_eq!(
        (code.as_deref(), message.as_deref()),
        (Some("MessageRejected"), Some(expected.as_str())),
        "case {:?}: expected {expected:?}, got {code:?} / {message:?}",
        past_the_quota.case
    );
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, SANDBOX_MAX_24_HOUR_SEND,
        "case after the refusal: the refused send must not be counted, got {sent}"
    );
}
