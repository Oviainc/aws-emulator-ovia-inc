// SPDX-License-Identifier: CC0-1.0

//! `SendCustomVerificationEmail`, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE the operation follows over a
//! SET of inputs, not one recorded request:
//!
//! * while the account has not been granted production access, EVERY request is
//!   refused with `ProductionAccessNotGranted` — whatever template,
//!   configuration set or address it names, and whether or not those exist — and
//!   nothing is read or written;
//! * once the account holds production access, the send resolves the named
//!   custom verification email template, requires the template's own sender to
//!   be a verified identity, answers with a message id it minted and stored, and
//!   adds the verified address to the ONE identity list every other identity
//!   operation reads;
//! * each cause the request itself carries — an unknown template, an unknown
//!   configuration set, an unverified template sender, an address that is not an
//!   address — is refused with the error modelled for that cause, and leaves the
//!   identity list and the send log alone.

use crate::harness::{call, element, elements, emulator_over, field, map_entries, xml_text, Field};
use service_com_amazonaws_ses_simpleemailservice::ops::types::CustomVerificationEmailTemplate;
use service_com_amazonaws_ses_simpleemailservice::state::{
    self, MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A router over an engine the test holds, so the account record and the custom
/// verification email template store can be seeded beside the calls made over
/// the wire. This service's model declares no operation that grants production
/// access, and `CreateCustomVerificationEmailTemplate` is still a scaffold stub
/// (its own issue), so both are seeded through the store.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    (emulator_over(Arc::clone(&engine)), engine)
}

/// Grant this account production access — what takes it out of the Amazon SES
/// sandbox, and so the one thing the account-wide gate reads.
fn grant_production_access(engine: &Arc<engine::InMemoryEngine>) {
    let store = state::service_state(&**engine).expect("this service has a store on the engine");
    store.lock().account.production_access_granted = true;
}

/// A custom verification email template, as the modelled shape the store holds.
fn template(name: &str, from: &str) -> CustomVerificationEmailTemplate {
    CustomVerificationEmailTemplate {
        template_name: Some(name.to_string()),
        from_email_address: Some(from.to_string()),
        template_subject: Some(format!("Please confirm your address ({name})")),
        success_redirection_url: Some("https://example.com/verified".to_string()),
        failure_redirection_url: Some("https://example.com/failed".to_string()),
    }
}

/// Record `templates` in the store, the way the creating operation will.
fn seed_templates(engine: &Arc<engine::InMemoryEngine>, templates: &[CustomVerificationEmailTemplate]) {
    let store = state::service_state(&**engine).expect("this service has a store on the engine");
    let mut store = store.lock();
    for template in templates {
        assert!(
            store
                .put_custom_verification_email_template(template.clone())
                .is_some(),
            "seeding {:?}: the store must hold it",
            template.template_name
        );
    }
}

/// `SendCustomVerificationEmail`.
async fn send_custom_verification_email(
    router: &MultiServiceRouter,
    address: &str,
    template_name: &str,
    configuration_set: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<Field> = vec![
        field("EmailAddress", address),
        field("TemplateName", template_name),
    ];
    if let Some(configuration_set) = configuration_set {
        params.push(field("ConfigurationSetName", configuration_set));
    }
    call(router, "SendCustomVerificationEmail", &params).await
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (element(body, "Code"), element(body, "Message"))
}

/// One call that must succeed, as its XML body.
async fn ok(router: &MultiServiceRouter, action: &str, params: &[Field]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {action}: expected 200, got {status} with {body}"
    );
    body
}

/// `ListIdentities`, as the identities it reports.
async fn identities(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "ListIdentities", &[]).await;
    element(&body, "Identities")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

/// `GetIdentityVerificationAttributes`, as the `VerificationStatus` it reports
/// for one identity.
async fn verification_status(router: &MultiServiceRouter, identity: &str) -> Option<String> {
    let body = ok(
        router,
        "GetIdentityVerificationAttributes",
        &[field("Identities.member.1", identity)],
    )
    .await;
    map_entries(&body)
        .into_iter()
        .find(|(key, _)| key == identity)
        .and_then(|(_, value)| element(&value, "VerificationStatus"))
}

/// `GetSendQuota`'s `SentLast24Hours`.
async fn sent_last_24_hours(router: &MultiServiceRouter) -> f64 {
    let body = ok(router, "GetSendQuota", &[]).await;
    let text = element(&body, "SentLast24Hours")
        .unwrap_or_else(|| panic!("GetSendQuota must report SentLast24Hours, got {body}"));
    text.parse::<f64>()
        .unwrap_or_else(|_| panic!("SentLast24Hours must be a number, got {text:?}"))
}

/// The name of the template every row below names when it names a stored one.
const STORED_TEMPLATE: &str = "confirm-your-address";

/// A template whose own sender is NOT a verified identity.
const UNVERIFIED_TEMPLATE: &str = "confirm-from-a-stranger";

#[tokio::test]
async fn the_production_access_gate_refuses_every_request_before_anything_it_names() {
    let (router, engine) = emulator_with_engine();
    seed_templates(
        &engine,
        &[
            template(STORED_TEMPLATE, "verification@example.com"),
            template(UNVERIFIED_TEMPLATE, "stranger@example.net"),
        ],
    );
    ok(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "verification@example.com")],
    )
    .await;
    ok(
        &router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", "tracked")],
    )
    .await;
    let before = identities(&router).await;

    // The rows differ in exactly what the request NAMES: a template that is
    // stored and one that is not, a template whose sender is verified and one
    // whose sender is not, no configuration set / one that exists / one that does
    // not, and an address that is not an address at all. Every one of them is a
    // cause this operation models its own error for — and none of them is
    // reached, because the account-wide gate is read first.
    let rows: &[(&str, &str, &str, Option<&str>)] = &[
        (
            "stored template, verified sender",
            "new@example.org",
            STORED_TEMPLATE,
            None,
        ),
        (
            "unknown template",
            "new@example.org",
            "no-such-template",
            None,
        ),
        (
            "stored template, unverified sender",
            "new@example.org",
            UNVERIFIED_TEMPLATE,
            None,
        ),
        (
            "existing configuration set",
            "new@example.org",
            STORED_TEMPLATE,
            Some("tracked"),
        ),
        (
            "unknown configuration set",
            "new@example.org",
            STORED_TEMPLATE,
            Some("no-such-set"),
        ),
        (
            "address that is not an address",
            "not-an-address",
            STORED_TEMPLATE,
            None,
        ),
        ("empty template name", "new@example.org", "", None),
    ];
    for (case, address, template_name, configuration_set) in rows {
        let (status, body) =
            send_custom_verification_email(&router, address, template_name, *configuration_set)
                .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 while the account is in the sandbox, got {status} with \
             {body}"
        );
        assert_eq!(
            error(&body),
            (
                Some("ProductionAccessNotGranted".to_string()),
                Some("The account has not been granted production access.".to_string())
            ),
            "case {case:?}: expected the account-wide refusal, got {body}"
        );
        // Refused means nothing happened: no identity was added for the address
        // the request named, and no message was logged.
        let after = identities(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the identity list untouched, got {after:?}"
        );
        let sent = sent_last_24_hours(&router).await;
        assert_eq!(
            sent, 0.0,
            "case {case:?}: expected nothing logged, got {sent}"
        );
    }
}

#[tokio::test]
async fn a_granted_account_sends_the_named_template_and_records_the_address() {
    let (router, engine) = emulator_with_engine();
    grant_production_access(&engine);
    // Two senders, spanning the two ways SES accepts a sender: the mailbox is a
    // verified identity of its own, and the second template's mailbox sits under
    // a verified DOMAIN, which covers the mailboxes in it and its subdomains.
    seed_templates(
        &engine,
        &[
            template(STORED_TEMPLATE, "verification@example.com"),
            template("confirm-under-a-domain", "noreply@mail.example.net"),
        ],
    );
    ok(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "verification@example.com")],
    )
    .await;
    ok(
        &router,
        "VerifyDomainIdentity",
        &[field("Domain", "example.net")],
    )
    .await;
    ok(
        &router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", "tracked")],
    )
    .await;

    // The rows differ along what the rule generalises over: which stored
    // template the send names (and so which verified sender it is sent from),
    // the form of the address being verified, and whether the send names a
    // configuration set.
    let rows: &[(&str, &str, &str, Option<&str>)] = &[
        (
            "plain mailbox",
            "new@example.org",
            STORED_TEMPLATE,
            None,
        ),
        (
            "dotted local part with a tag",
            "first.last+ses@example.org",
            STORED_TEMPLATE,
            Some("tracked"),
        ),
        (
            "mailbox under a subdomain, sender covered by a verified domain",
            "ops@mail.corp.example.co.uk",
            "confirm-under-a-domain",
            None,
        ),
        (
            "shortest labels that are still an address",
            "a@e.io",
            "confirm-under-a-domain",
            Some("tracked"),
        ),
        (
            "display-name form around the addr-spec",
            "New Joiner <joiner@example.org>",
            STORED_TEMPLATE,
            None,
        ),
    ];
    let mut message_ids: Vec<String> = Vec::new();
    for (index, (case, address, template_name, configuration_set)) in rows.iter().enumerate() {
        let before = sent_last_24_hours(&router).await;
        let (status, body) =
            send_custom_verification_email(&router, address, template_name, *configuration_set)
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 once the account holds production access, got {status} \
             with {body}"
        );
        let message_id = element(&body, "MessageId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a MessageId, got {body}"));
        assert!(
            !message_id.is_empty() && !message_ids.contains(&message_id),
            "case {case:?}: expected a message id this account had not answered with before, got \
             {message_id:?} after {message_ids:?}"
        );
        message_ids.push(message_id);

        // The address is now in the ONE identity list, read back through the
        // operations that report it — the round trip, over the wire.
        let key = address
            .rsplit_once('<')
            .map_or(*address, |(_, bracketed)| {
                bracketed.trim_end_matches('>')
            });
        let listed = identities(&router).await;
        assert!(
            listed.contains(&key.to_string()),
            "case {case:?}: expected {key:?} in the identity list, got {listed:?}"
        );
        let reported = verification_status(&router, key).await;
        assert_eq!(
            reported.as_deref(),
            Some("Success"),
            "case {case:?}: expected the stored verification status for {key:?}, got {reported:?}"
        );
        // The accepted message is in the send log the quota is derived from: one
        // recipient, so one more than before.
        let after = sent_last_24_hours(&router).await;
        assert_eq!(
            after,
            before + 1.0,
            "case {case:?}: expected one more message in the 24-hour window after send {}, got \
             {after}",
            index + 1
        );
    }
}

#[tokio::test]
async fn a_granted_account_refuses_each_cause_the_request_carries_with_its_own_error() {
    let (router, engine) = emulator_with_engine();
    grant_production_access(&engine);
    seed_templates(
        &engine,
        &[
            template(STORED_TEMPLATE, "verification@example.com"),
            template(UNVERIFIED_TEMPLATE, "stranger@example.net"),
        ],
    );
    ok(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "verification@example.com")],
    )
    .await;
    let before = identities(&router).await;

    // One row per cause this operation models an error for, each named by a
    // request that carries only that cause.
    let rows: &[(&str, &str, &str, Option<&str>, &str)] = &[
        (
            "unknown template",
            "new@example.org",
            "no-such-template",
            None,
            "CustomVerificationEmailTemplateDoesNotExist",
        ),
        (
            "empty template name names no template",
            "new@example.org",
            "",
            None,
            "CustomVerificationEmailTemplateDoesNotExist",
        ),
        (
            "template sender is not verified",
            "new@example.org",
            UNVERIFIED_TEMPLATE,
            None,
            "FromEmailAddressNotVerified",
        ),
        (
            "unknown configuration set",
            "new@example.org",
            STORED_TEMPLATE,
            Some("no-such-set"),
            "ConfigurationSetDoesNotExist",
        ),
        (
            "address that is not an address",
            "not-an-address",
            STORED_TEMPLATE,
            None,
            "MessageRejected",
        ),
        (
            "address with no domain labels",
            "nobody@localhost",
            STORED_TEMPLATE,
            None,
            "MessageRejected",
        ),
    ];
    for (case, address, template_name, configuration_set, expected) in rows {
        let (status, body) =
            send_custom_verification_email(&router, address, template_name, *configuration_set)
                .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some(*expected),
            "case {case:?}: expected the error modelled for this cause, got {code:?} / {message:?}"
        );
        let after = identities(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the identity list untouched, got {after:?}"
        );
        let sent = sent_last_24_hours(&router).await;
        assert_eq!(
            sent, 0.0,
            "case {case:?}: expected nothing logged, got {sent}"
        );
    }
}

#[tokio::test]
async fn the_custom_verification_email_template_store_is_bounded() {
    let (router, engine) = emulator_with_engine();
    grant_production_access(&engine);
    let full: Vec<CustomVerificationEmailTemplate> = (0..MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES)
        .map(|slot| template(&format!("template-{slot}"), "verification@example.com"))
        .collect();
    seed_templates(&engine, &full);
    ok(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "verification@example.com")],
    )
    .await;

    // The store is at its ceiling, so a further NEW template is not recorded —
    // while replacing one of the names already stored is, since it adds nothing.
    {
        let store =
            state::service_state(&*engine).expect("this service has a store on the engine");
        let mut store = store.lock();
        assert!(
            store
                .put_custom_verification_email_template(template(
                    "one-too-many",
                    "verification@example.com"
                ))
                .is_none(),
            "case store at its ceiling: expected a new template to be refused, got {} stored",
            store.custom_verification_email_templates.len()
        );
        assert!(
            store
                .put_custom_verification_email_template(template(
                    "template-0",
                    "verification@example.com"
                ))
                .is_some(),
            "case replacing a stored name: expected it to be recorded"
        );
    }

    // What the store holds is what a send can name: the template that was
    // refused names nothing, and the ones inside the ceiling send.
    let (status, body) =
        send_custom_verification_email(&router, "new@example.org", "one-too-many", None).await;
    assert_eq!(
        status, 400,
        "case template past the ceiling: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error(&body).0.as_deref(),
        Some("CustomVerificationEmailTemplateDoesNotExist"),
        "case template past the ceiling: expected the modelled absence, got {body}"
    );
    for name in ["template-0", &format!("template-{}", MAX_CUSTOM_VERIFICATION_EMAIL_TEMPLATES - 1)] {
        let (status, body) =
            send_custom_verification_email(&router, "new@example.org", name, None).await;
        assert_eq!(
            status, 200,
            "case stored template {name:?}: expected 200, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn both_sends_word_an_illegal_address_the_same_way() {
    let (router, engine) = emulator_with_engine();
    grant_production_access(&engine);
    seed_templates(
        &engine,
        &[template(STORED_TEMPLATE, "verification@example.com")],
    );
    ok(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "verification@example.com")],
    )
    .await;

    // The two operations that take a caller-supplied address read it the same way
    // and report it the same way: the refusal comes from the one shared wording,
    // so neither operation can drift from the other. The rows span the forms a
    // value that is not an address takes.
    for malformed in ["not-an-address", "nobody@localhost", "@example.com", ""] {
        let (status, body) =
            send_custom_verification_email(&router, malformed, STORED_TEMPLATE, None).await;
        assert_eq!(
            status, 400,
            "case {malformed:?} to SendCustomVerificationEmail: expected 400, got {status} with \
             {body}"
        );
        let (verification_code, verification_message) = error(&body);

        let (status, body) = call(
            &router,
            "SendEmail",
            &[
                field("Source", "verification@example.com"),
                field("Destination.ToAddresses.member.1", malformed),
                field("Message.Subject.Data", "Quarterly report"),
                field("Message.Body.Text.Data", "See attached."),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {malformed:?} to SendEmail: expected 400, got {status} with {body}"
        );
        let (send_code, send_message) = error(&body);

        assert_eq!(
            (verification_code, verification_message.map(|m| xml_text(&m))),
            (send_code, send_message.map(|m| xml_text(&m))),
            "case {malformed:?}: expected both sends to refuse it identically"
        );
    }
}
