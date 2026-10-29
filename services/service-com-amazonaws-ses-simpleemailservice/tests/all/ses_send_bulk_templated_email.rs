// SPDX-License-Identifier: CC0-1.0

//! `SendBulkTemplatedEmail` end to end over the wire, and the readers that prove
//! what it recorded.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each call
//! is a real awsQuery request against the service: form-urlencoded body in, XML
//! out, one shared engine behind it. What is asserted is the RULE the operation
//! follows over a SET of inputs rather than one recorded request:
//!
//! * a bulk send composes ONE message per destination from one of the account's
//!   stored email templates, reports one status per destination in request order,
//!   and counts every recipient of every accepted message against the account's
//!   24-hour sending quota — which is what reads the sends back;
//! * a condition of the CALL — the account's sending state, the configuration set
//!   it names, the sending identity, the template it names, the number of
//!   destinations it carries — refuses the whole call with that condition's own
//!   modelled error and records nothing;
//! * a condition of ONE destination — its recipient set, or the account's quota
//!   running out part way through the call — is reported as THAT destination's
//!   status, leaving the destinations beside it accepted.
//!
//! The rows differ along what the rule generalises over: how many destinations a
//! call carries (one, several, the documented ceiling), which recipient fields
//! each destination uses and how many addresses they hold (up to the documented
//! ceiling), which parts the stored template carries, what the replacement values
//! and tags are (none, per-call defaults, per-destination replacements), whether
//! the call names a configuration set, and which gate a row is built to fail.

use crate::harness::{
    call, element, elements, emulator_and_engine, error_envelope, field, settle_verifications,
    xml_text, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::{
    DEFAULT_REGION, MAX_BULK_DESTINATIONS, MAX_RECIPIENTS, SANDBOX_MAX_24_HOUR_SEND,
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

/// The configuration set a call may name, created with sending enabled.
const LIVE_SET: &str = "live-bulk-set";

/// A configuration set the account holds with its sending PAUSED.
const PAUSED_SET: &str = "paused-bulk-set";

/// The template every accepted row below composes from, unless the row names
/// another one.
const DEFAULT_TEMPLATE: &str = "every-part";

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
            name: DEFAULT_TEMPLATE,
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

/// One `BulkEmailDestination` of a call: the recipient set one message of the
/// bulk send goes to, with the replacement values and tags that destination
/// carries.
#[derive(Debug, Clone, Default)]
struct Dest {
    /// `Destination.ToAddresses`.
    to: Vec<String>,
    /// `Destination.CcAddresses`.
    cc: Vec<String>,
    /// `Destination.BccAddresses`.
    bcc: Vec<String>,
    /// `ReplacementTemplateData`, when this destination carries its own.
    replacement_template_data: Option<String>,
    /// `ReplacementTags`, in request order.
    replacement_tags: Vec<(String, String)>,
}

impl Dest {
    /// A destination addressing `to` in the To: field alone.
    fn to(addresses: &[&str]) -> Self {
        Self {
            to: addresses.iter().map(|address| address.to_string()).collect(),
            ..Self::default()
        }
    }

    /// Every recipient of this destination, across the three recipient fields —
    /// the count that is charged against the sending quota.
    fn recipient_count(&self) -> usize {
        self.to.len() + self.cc.len() + self.bcc.len()
    }

    /// The form fields this destination contributes, as the `index`-th (0-based)
    /// member of the request's `Destinations` list.
    fn fields(&self, index: usize) -> Vec<Field> {
        let at = format!("Destinations.member.{}", index + 1);
        let mut params = Vec::new();
        for (member, addresses) in [
            ("ToAddresses", &self.to),
            ("CcAddresses", &self.cc),
            ("BccAddresses", &self.bcc),
        ] {
            if addresses.is_empty() {
                // `BulkEmailDestination$Destination` is `@required`, so it must
                // be on the wire even when it names nobody: the bare list key
                // carries the member with no elements, which is how a caller
                // asks SES to send to nobody.
                params.push(field(format!("{at}.Destination.{member}"), ""));
                continue;
            }
            for (position, address) in addresses.iter().enumerate() {
                params.push(field(
                    format!("{at}.Destination.{member}.member.{}", position + 1),
                    address.clone(),
                ));
            }
        }
        if let Some(data) = &self.replacement_template_data {
            params.push(field(format!("{at}.ReplacementTemplateData"), data.clone()));
        }
        for (position, (name, value)) in self.replacement_tags.iter().enumerate() {
            let tag = format!("{at}.ReplacementTags.member.{}", position + 1);
            params.push(field(format!("{tag}.Name"), name.clone()));
            params.push(field(format!("{tag}.Value"), value.clone()));
        }
        params
    }
}

/// One `SendBulkTemplatedEmail` request.
struct Bulk {
    /// What makes this row different from the others.
    case: String,
    /// The `Source` every message of the call is sent from.
    source: String,
    /// The `Template` every message of the call is composed from.
    template: String,
    /// `DefaultTemplateData`, which the model requires.
    default_template_data: String,
    /// `ConfigurationSetName`, when the call names one.
    configuration_set: Option<String>,
    /// `DefaultTags`, in request order.
    default_tags: Vec<(String, String)>,
    /// The destinations, in request order.
    destinations: Vec<Dest>,
}

impl Bulk {
    /// A call from [`SENDER`] of `template` to `destinations`, which a row then
    /// varies.
    fn new(case: &str, template: &str, destinations: Vec<Dest>) -> Self {
        Self {
            case: case.to_string(),
            source: SENDER.to_string(),
            template: template.to_string(),
            default_template_data: "{}".to_string(),
            configuration_set: None,
            default_tags: Vec::new(),
            destinations,
        }
    }

    /// Every recipient this call addresses, across every destination of it.
    fn recipient_count(&self) -> usize {
        self.destinations
            .iter()
            .map(Dest::recipient_count)
            .sum::<usize>()
    }

    /// The form fields that carry this call on the wire.
    fn fields(&self) -> Vec<Field> {
        let mut params = vec![
            field("Source", self.source.clone()),
            field("Template", self.template.clone()),
            field("DefaultTemplateData", self.default_template_data.clone()),
        ];
        if let Some(name) = &self.configuration_set {
            params.push(field("ConfigurationSetName", name.clone()));
        }
        for (index, (name, value)) in self.default_tags.iter().enumerate() {
            params.push(field(
                format!("DefaultTags.member.{}.Name", index + 1),
                name.clone(),
            ));
            params.push(field(
                format!("DefaultTags.member.{}.Value", index + 1),
                value.clone(),
            ));
        }
        for (index, destination) in self.destinations.iter().enumerate() {
            params.extend(destination.fields(index));
        }
        params
    }
}

/// One reported `BulkEmailDestinationStatus`: `(Status, Error, MessageId)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    status: Option<String>,
    error: Option<String>,
    message_id: Option<String>,
}

/// The `Status` list a bulk send answered with, one entry per destination in
/// request order.
///
/// The list element and each entry's own status member are both named `Status`,
/// so the `<member>` entries are read out of the response directly: they are the
/// only list entries this operation's response carries.
fn reported(body: &str) -> Vec<Reported> {
    elements(body, "member")
        .into_iter()
        .map(|entry| Reported {
            status: element(&entry, "Status"),
            error: element(&entry, "Error").map(|text| xml_text(&text)),
            message_id: element(&entry, "MessageId"),
        })
        .collect()
}

/// `recipient-<n>@example.net`, so a row can address as many recipients as it
/// needs in a domain this account has verified.
fn recipients(count: usize) -> Vec<String> {
    (1..=count)
        .map(|index| format!("recipient-{index}@{RECIPIENT_DOMAIN}"))
        .collect()
}

/// `count` destinations of one recipient each, which is how a row spans the
/// number of destinations one call carries.
fn one_recipient_each(count: usize) -> Vec<Dest> {
    recipients(count)
        .into_iter()
        .map(|address| Dest {
            to: vec![address],
            ..Dest::default()
        })
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

/// The delivery attempts `GetSendStatistics` reports across every data point,
/// which is the other reader of the send log the accepted messages are written
/// to.
async fn delivery_attempts(router: &MultiServiceRouter) -> u64 {
    let (status, body) = call(router, "GetSendStatistics", &[]).await;
    assert_eq!(
        status, 200,
        "case GetSendStatistics: expected 200, got {status} with {body}"
    );
    elements(&body, "DeliveryAttempts")
        .iter()
        .map(|text| {
            text.parse::<u64>()
                .unwrap_or_else(|_| panic!("case GetSendStatistics: expected a number, got {text:?}"))
        })
        .sum()
}

/// The `Template` element `GetTemplate` reports for `name`, as its raw XML, so a
/// case can assert a bulk send left the stored template exactly as it was.
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
async fn every_form_of_bulk_send_is_accepted_per_destination_and_counted() {
    let (router, _engine) = emulator_ready().await;

    // The rows span what the rule generalises over: how many destinations the
    // call carries, which recipient fields each destination uses and how many
    // addresses they hold, which template the call composes from, what the
    // replacement values and tags are, and whether the call names a
    // configuration set.
    let mut rows = vec![
        Bulk::new(
            "one destination, one To: recipient",
            DEFAULT_TEMPLATE,
            vec![Dest::to(&["a@example.net"])],
        ),
        Bulk::new(
            "three destinations, a template with no HTML part",
            "text-only",
            one_recipient_each(3),
        ),
    ];
    // Recipients in the CC: field alone, in the BCC: field alone, and across all
    // three — the ceiling is "across the To:, CC: and BCC: fields", so each field
    // is a recipient field like the others.
    let cc_only = Dest {
        cc: recipients(2),
        ..Dest::default()
    };
    let bcc_only = Dest {
        bcc: recipients(1),
        ..Dest::default()
    };
    let mixed = Dest {
        to: vec!["e@example.net".to_string()],
        cc: vec!["f@example.net".to_string()],
        bcc: vec!["g@example.net".to_string()],
        ..Dest::default()
    };
    rows.push(Bulk::new(
        "destinations addressing each recipient field, a template with no text part",
        "html-only",
        vec![cc_only, bcc_only, mixed],
    ));
    // The documented recipient ceiling of ONE message: "The message may not
    // include more than 50 recipients, across the To:, CC: and BCC: fields."
    let at_the_recipient_ceiling = Dest {
        to: recipients(MAX_RECIPIENTS - 2),
        cc: recipients(1),
        bcc: vec!["last@example.net".to_string()],
        ..Dest::default()
    };
    rows.push(Bulk::new(
        "one destination at the documented recipient ceiling, a template with a subject only",
        "subject-only",
        vec![at_the_recipient_ceiling],
    ));
    // ... and the documented ceiling on the DESTINATIONS of one call: "You can
    // specify up to 50 Destination objects within a Destinations array."
    rows.push(Bulk::new(
        "the documented destination ceiling",
        DEFAULT_TEMPLATE,
        one_recipient_each(MAX_BULK_DESTINATIONS),
    ));
    // The replacement values: a destination falling back on the call's
    // `DefaultTemplateData`, one carrying flat pairs of its own, and one
    // carrying a nested document.
    rows.push(Bulk::new(
        "replacement values: none, flat, nested",
        DEFAULT_TEMPLATE,
        vec![
            Dest::to(&["values-default@example.net"]),
            Dest {
                to: vec!["values-flat@example.net".to_string()],
                replacement_template_data: Some(r#"{"name":"Ada","order":"4711"}"#.to_string()),
                ..Dest::default()
            },
            Dest {
                to: vec!["values-nested@example.net".to_string()],
                replacement_template_data: Some(
                    r#"{"name":"Ada","order":{"id":"4711","items":["pen","ink"]}}"#.to_string(),
                ),
                ..Dest::default()
            },
        ],
    ));
    // The tags: the call's defaults, and a destination replacing them with its
    // own.
    let mut tagged = Bulk::new(
        "default tags, and a destination replacing them",
        DEFAULT_TEMPLATE,
        vec![
            Dest::to(&["tags-default@example.net"]),
            Dest {
                to: vec!["tags-replaced@example.net".to_string()],
                replacement_tags: vec![("campaign".to_string(), "autumn".to_string())],
                ..Dest::default()
            },
        ],
    );
    tagged.default_tags = vec![
        ("campaign".to_string(), "spring".to_string()),
        ("segment".to_string(), "new".to_string()),
    ];
    rows.push(tagged);
    // A call naming a configuration set the account holds.
    let mut with_set = Bulk::new(
        "a configuration set the account holds",
        DEFAULT_TEMPLATE,
        vec![Dest::to(&["set@example.net"])],
    );
    with_set.configuration_set = Some(LIVE_SET.to_string());
    rows.push(with_set);
    // A display-name source resolves to the same mailbox the account verified.
    let mut display_name = Bulk::new(
        "a Source in display-name form",
        DEFAULT_TEMPLATE,
        vec![Dest::to(&["display@example.net"])],
    );
    display_name.source = format!("Sales Desk <{SENDER}>");
    rows.push(display_name);

    let mut counted = 0.0;
    let mut ids: Vec<String> = Vec::new();
    for row in &rows {
        let (status, body) = call(&router, "SendBulkTemplatedEmail", &row.fields()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body}",
            row.case
        );
        assert!(
            body.contains("<SendBulkTemplatedEmailResponse"),
            "case {:?}: expected this operation's awsQuery envelope, got {body}",
            row.case
        );
        let statuses = reported(&body);
        assert_eq!(
            statuses.len(),
            row.destinations.len(),
            "case {:?}: expected one status per destination, got {statuses:?}",
            row.case
        );
        for (position, entry) in statuses.iter().enumerate() {
            assert_eq!(
                (entry.status.as_deref(), entry.error.as_deref()),
                (Some("Success"), None),
                "case {:?} destination {position}: expected an accepted destination, got {entry:?}",
                row.case
            );
            let message_id = entry.message_id.clone().unwrap_or_default();
            assert!(
                !message_id.is_empty(),
                "case {:?} destination {position}: an accepted destination reports the id it \
                 stored, got {entry:?}",
                row.case
            );
            assert!(
                !ids.contains(&message_id),
                "case {:?} destination {position}: expected an id no earlier message answered \
                 with, got {message_id:?} among {ids:?}",
                row.case
            );
            ids.push(message_id);
        }

        // Every recipient of every accepted message counts once against the
        // account's 24-hour quota, which is what reads the send log back.
        counted += row.recipient_count() as f64;
        let sent = sent_last_24_hours(&router).await;
        assert_eq!(
            sent, counted,
            "case {:?}: expected {counted} counted recipients after the call, got {sent}",
            row.case
        );
    }

    // The other reader of the same log agrees with the quota: each recipient of
    // an accepted message is one delivery attempt.
    let attempts = delivery_attempts(&router).await;
    assert_eq!(
        attempts as f64, counted,
        "case the whole suite: expected {counted} delivery attempts reported, got {attempts}"
    );

    // The calls composed from the stored templates and changed none of them.
    for template in templates() {
        let reported_template = stored_template(&router, template.name).await;
        let reported_template = reported_template.unwrap_or_else(|| {
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
            let part = element(&reported_template, member).map(|text| xml_text(&text));
            assert_eq!(
                part.as_deref(),
                stored,
                "case {} {member}: expected the stored part to survive the sends, got \
                 {reported_template}",
                template.name
            );
        }
    }
}

#[tokio::test]
async fn a_condition_of_the_whole_bulk_call_refuses_it_with_its_own_error() {
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

    /// A row: the call, and the modelled code and message it is refused with.
    struct Refusal {
        call: Bulk,
        code: &'static str,
        message: String,
    }

    const MISSING: &str = "no-such-template";
    let mut rows: Vec<Refusal> = Vec::new();

    // One template composes every message of the call, so a template the account
    // does not hold refuses the CALL — and it is resolved LAST of the call's
    // checks.
    rows.push(Refusal {
        call: Bulk::new(
            "a template the account does not hold",
            MISSING,
            vec![Dest::to(&["a@example.net"])],
        ),
        code: "TemplateDoesNotExist",
        message: format!("Template {MISSING} does not exist."),
    });

    // ... and the same call from a sender this account has not verified is
    // refused for the IDENTITY, whichever template it named: the identity gate is
    // ahead of the template lookup.
    for (case, source) in [
        (
            "a sender whose verification was only requested",
            PENDING_SENDER,
        ),
        ("a sender nobody ever asked about", "stranger@example.com"),
    ] {
        for template in [MISSING, DEFAULT_TEMPLATE] {
            let mut row = Bulk::new(
                &format!("{case}, naming the template {template}"),
                template,
                vec![Dest::to(&["a@example.net"])],
            );
            row.source = source.to_string();
            rows.push(Refusal {
                call: row,
                code: "MessageRejected",
                message: unverified_message(source),
            });
        }
    }

    // The configuration set the call names is read ahead of the identity and the
    // template both.
    let mut unknown_set = Bulk::new(
        "a configuration set the account does not hold, an unverified sender and a missing \
         template",
        MISSING,
        vec![Dest::to(&["a@example.net"])],
    );
    unknown_set.source = PENDING_SENDER.to_string();
    unknown_set.configuration_set = Some("no-such-set".to_string());
    rows.push(Refusal {
        call: unknown_set,
        code: "ConfigurationSetDoesNotExist",
        message: "Configuration set <no-such-set> does not exist.".to_string(),
    });
    let mut paused_set = Bulk::new(
        "a configuration set whose sending is paused, an unverified sender and a missing template",
        MISSING,
        vec![Dest::to(&["a@example.net"])],
    );
    paused_set.source = PENDING_SENDER.to_string();
    paused_set.configuration_set = Some(PAUSED_SET.to_string());
    rows.push(Refusal {
        call: paused_set,
        code: "ConfigurationSetSendingPausedException",
        message: format!("Email sending is disabled for configuration set <{PAUSED_SET}>."),
    });

    // The number of destinations the request carries is read FIRST, before
    // anything the account holds: one destination past the documented ceiling is
    // refused for the request's own shape even when the set, the sender and the
    // template would all have failed too.
    let mut past_the_ceiling = Bulk::new(
        "one destination past the documented ceiling",
        MISSING,
        one_recipient_each(MAX_BULK_DESTINATIONS + 1),
    );
    past_the_ceiling.source = PENDING_SENDER.to_string();
    past_the_ceiling.configuration_set = Some("no-such-set".to_string());
    rows.push(Refusal {
        call: past_the_ceiling,
        code: "MessageRejected",
        message: format!(
            "Destinations may include at most {MAX_BULK_DESTINATIONS} Destination objects, got {}",
            MAX_BULK_DESTINATIONS + 1
        ),
    });

    for row in &rows {
        let (status, body) = call(&router, "SendBulkTemplatedEmail", &row.call.fields()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {body}",
            row.call.case
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (code.as_deref(), message.as_deref()),
            (Some(row.code), Some(row.message.as_str())),
            "case {:?}: expected {:?} / {:?}, got {code:?} / {message:?}",
            row.call.case,
            row.code,
            row.message
        );
    }

    // No refused call reached the send log the quota is derived from, and none of
    // them created the configuration set or the template it named.
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, 0.0,
        "case after the refusals: expected an untouched send log, got {sent}"
    );
    assert!(
        stored_template(&router, MISSING).await.is_none(),
        "case after the refusals: a refused call must not create the template it named"
    );
    let (status, body) = call(
        &router,
        "DescribeConfigurationSet",
        &[field("ConfigurationSetName", "no-such-set")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case after the refusals: a refused call must not create the set it named, got {status} \
         with {body}"
    );
}

#[tokio::test]
async fn a_destination_this_service_cannot_send_to_is_reported_on_its_own() {
    let (router, _engine) = emulator_ready().await;

    /// A destination of the mixed call below, and the status it is reported with.
    struct Row {
        case: &'static str,
        destination: Dest,
        status: &'static str,
        error: Option<String>,
    }

    let accepted = |case: &'static str, address: &'static str| Row {
        case,
        destination: Dest::to(&[address]),
        status: "Success",
        error: None,
    };
    let mut rows = vec![accepted("a destination that can be sent to", "a@example.net")];
    // "Each Destination parameter must include at least one recipient email
    // address."
    rows.push(Row {
        case: "a destination with no recipient at all",
        destination: Dest::default(),
        status: "InvalidParameterValue",
        error: Some(
            "Destination must include at least one recipient address, across the ToAddresses, \
             CcAddresses and BccAddresses fields"
                .to_string(),
        ),
    });
    // "If a recipient email address is invalid (that is, it is not in the format
    // UserName@[SubDomain.]Domain.TopLevelDomain), the entire message is
    // rejected, even if the message contains other recipients that are valid."
    for (case, invalid) in [
        ("a recipient with no domain", "nobody"),
        ("a recipient with no local part", "@example.net"),
        ("a recipient with a doubled dot", "first..last@example.net"),
    ] {
        rows.push(Row {
            case,
            destination: Dest {
                to: vec!["valid@example.net".to_string()],
                cc: vec![invalid.to_string()],
                ..Dest::default()
            },
            status: "InvalidParameterValue",
            error: Some(format!(
                "Illegal address: {invalid:?} is not of the form \
                 UserName@[SubDomain.]Domain.TopLevelDomain"
            )),
        });
    }
    // One recipient past the documented ceiling of ONE message.
    rows.push(Row {
        case: "a destination one recipient past the documented ceiling",
        destination: Dest {
            to: recipients(MAX_RECIPIENTS + 1),
            ..Dest::default()
        },
        status: "InvalidParameterValue",
        error: Some(format!(
            "Destination may include at most {MAX_RECIPIENTS} recipients across the ToAddresses, \
             CcAddresses and BccAddresses fields, got {}",
            MAX_RECIPIENTS + 1
        )),
    });
    rows.push(accepted(
        "a destination that can be sent to, after the ones that cannot",
        "z@example.net",
    ));

    let row_call = Bulk::new(
        "destinations this service can and cannot send to, in one call",
        DEFAULT_TEMPLATE,
        rows.iter().map(|row| row.destination.clone()).collect(),
    );
    let (status, body) = call(&router, "SendBulkTemplatedEmail", &row_call.fields()).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {body}",
        row_call.case
    );
    let statuses = reported(&body);
    assert_eq!(
        statuses.len(),
        rows.len(),
        "case {:?}: expected one status per destination, got {statuses:?}",
        row_call.case
    );
    // "Note that order of responses will be respective to order of destinations
    // in the request."
    for (row, entry) in rows.iter().zip(&statuses) {
        assert_eq!(
            (entry.status.as_deref(), entry.error.as_deref()),
            (Some(row.status), row.error.as_deref()),
            "case {:?}: expected {:?} / {:?}, got {entry:?}",
            row.case,
            row.status,
            row.error
        );
        assert_eq!(
            entry.message_id.is_some(),
            row.status == "Success",
            "case {:?}: only an accepted destination reports a message id, got {entry:?}",
            row.case
        );
    }

    // Only the accepted destinations reached the send log: one recipient each.
    let accepted_recipients = rows
        .iter()
        .filter(|row| row.status == "Success")
        .map(|row| row.destination.recipient_count())
        .sum::<usize>() as f64;
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, accepted_recipients,
        "case {:?}: expected {accepted_recipients} counted recipients, got {sent}",
        row_call.case
    );
}

#[tokio::test]
async fn a_bulk_call_is_refused_while_the_account_is_paused() {
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

    let mut paused = Bulk::new(
        "an unverified sender and a missing template while the account is paused",
        "no-such-template",
        vec![Dest::to(&["a@example.net"])],
    );
    paused.source = "stranger@example.com".to_string();
    paused.configuration_set = Some("no-such-set".to_string());
    let rows = [
        Bulk::new(
            "a call that would otherwise be accepted",
            DEFAULT_TEMPLATE,
            one_recipient_each(2),
        ),
        paused,
    ];
    for row in &rows {
        let (status, body) = call(&router, "SendBulkTemplatedEmail", &row.fields()).await;
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

    // Enabling sending again serves the same call, so the refusal was about the
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
    let (status, body) = call(&router, "SendBulkTemplatedEmail", &rows[0].fields()).await;
    assert_eq!(
        status, 200,
        "case once sending is enabled again: expected 200, got {status} with {body}"
    );
    let statuses = reported(&body);
    assert!(
        statuses
            .iter()
            .all(|entry| entry.status.as_deref() == Some("Success")),
        "case once sending is enabled again: expected every destination accepted, got {statuses:?}"
    );
}

#[tokio::test]
async fn the_quota_running_out_part_way_through_a_call_stops_at_that_destination() {
    let (router, _engine) = emulator_ready().await;
    // Fill the account's stored sandbox quota to two recipients short of it, in
    // calls that are all accepted: each recipient of each destination counts
    // once.
    let quota = SANDBOX_MAX_24_HOUR_SEND as usize;
    let mut filled = 0;
    while filled + MAX_BULK_DESTINATIONS <= quota - 2 {
        let row = Bulk::new(
            "a call filling the quota",
            DEFAULT_TEMPLATE,
            one_recipient_each(MAX_BULK_DESTINATIONS),
        );
        let (status, body) = call(&router, "SendBulkTemplatedEmail", &row.fields()).await;
        assert_eq!(
            status, 200,
            "case quota fill at {filled}: expected 200, got {status} with {body}"
        );
        filled += MAX_BULK_DESTINATIONS;
    }
    let remaining = quota - 2 - filled;
    if remaining > 0 {
        let row = Bulk::new(
            "a call filling the last of the quota",
            DEFAULT_TEMPLATE,
            one_recipient_each(remaining),
        );
        let (status, body) = call(&router, "SendBulkTemplatedEmail", &row.fields()).await;
        assert_eq!(
            status, 200,
            "case quota fill remainder: expected 200, got {status} with {body}"
        );
    }
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent,
        (quota - 2) as f64,
        "case the filled quota: expected {} counted recipients, got {sent}",
        quota - 2
    );

    // Two recipients of the quota are left, so the first destination of this
    // call fits exactly and every destination after it does not — the quota is
    // read against the recipients ACCEPTED so far, not the destinations.
    let crossing = Bulk::new(
        "a call crossing the quota part way through",
        DEFAULT_TEMPLATE,
        vec![
            Dest {
                to: recipients(2),
                ..Dest::default()
            },
            Dest::to(&["after-the-quota@example.net"]),
            Dest::to(&["also-after-the-quota@example.net"]),
        ],
    );
    let (status, body) = call(&router, "SendBulkTemplatedEmail", &crossing.fields()).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {body}",
        crossing.case
    );
    let statuses = reported(&body);
    let exceeded = format!(
        "Daily message quota exceeded: this account may send {SANDBOX_MAX_24_HOUR_SEND} messages \
         per 24 hours and has sent {SANDBOX_MAX_24_HOUR_SEND}"
    );
    let expected = [
        ("Success", None),
        ("AccountDailyQuotaExceeded", Some(exceeded.as_str())),
        ("AccountDailyQuotaExceeded", Some(exceeded.as_str())),
    ];
    assert_eq!(
        statuses.len(),
        expected.len(),
        "case {:?}: expected one status per destination, got {statuses:?}",
        crossing.case
    );
    for (position, (entry, (status, error))) in statuses.iter().zip(expected).enumerate() {
        assert_eq!(
            (entry.status.as_deref(), entry.error.as_deref()),
            (Some(status), error),
            "case {:?} destination {position}: expected {status:?} / {error:?}, got {entry:?}",
            crossing.case
        );
    }

    // The quota is exactly full: the destination that fit was counted and the
    // ones past it were not.
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, SANDBOX_MAX_24_HOUR_SEND,
        "case after the crossing: expected {SANDBOX_MAX_24_HOUR_SEND} counted recipients, got \
         {sent}"
    );
}
