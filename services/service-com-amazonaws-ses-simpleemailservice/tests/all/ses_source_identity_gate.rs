// SPDX-License-Identifier: CC0-1.0

//! The source-identity gate every send of this service passes, over the forms a
//! sending identity and a send can take.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request. The RULE asserted, over a SET of inputs rather than one
//! recorded request: a send is refused `MessageRejected` (HTTP 400) unless the
//! identity its message is sent FROM — the `Source` parameter, or the raw
//! message's own `From` line when the request names no `Source` — is one whose
//! verification this account has COMPLETED; asking for an identity's verification
//! only starts it, so the identity is not a sender until the move the clock owes
//! the record falls due (`state::IDENTITY_VERIFICATION_SPAN`). The refusal names
//! the Region the request was signed for and the addr-spec of the identity that
//! failed the check (`state::unverified_identities_message`).
//!
//! The rows differ along what the rule generalises over:
//!
//! * which operation carries the send — `SendEmail`, `SendRawEmail` with a
//!   `Source`, and `SendRawEmail` reading the message's own `From` line;
//! * which identity the source resolves to — the mailbox itself, the mailbox
//!   under a requested domain, a mailbox under a SUBDOMAIN of one, the same
//!   mailbox written in display-name form, and an identity nobody ever asked
//!   about;
//! * which spelling recorded it, the current one or the deprecated
//!   `VerifyEmailAddress`.
//!
//! The span itself is driven by the real clock in one case
//! (`the_move_out_of_pending_is_owed_by_the_clock`); the other cases settle the
//! store past it (`harness::settle_verifications`) rather than waiting, because
//! what they are about is the gate.

use crate::harness::{
    call, element, emulator_and_engine, field, map_entries, settle_verifications, xml_text, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::{
    DEFAULT_REGION, IDENTITY_VERIFICATION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The message a send from an identity this account has not verified is refused
/// with, as this service words it for the Region the harness signs for and one
/// identity.
fn expected_message(identity: &str) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in region \
         {}: {identity}",
        DEFAULT_REGION.to_uppercase()
    )
}

/// A raw message from `source` to `recipient`, with the blank line SES requires
/// between the headers and the body.
fn raw_message(source: &str, recipient: &str) -> String {
    format!(
        "From: {source}\r\nTo: {recipient}\r\nSubject: Quarterly report\r\n\r\nSee attached.\r\n"
    )
}

/// One send, as the request that carries it.
struct Send {
    /// What makes this row different from the others.
    case: &'static str,
    /// The operation the send goes through.
    action: &'static str,
    /// The parameters of that operation.
    params: Vec<Field>,
}

/// The same message sent from `source`, in each form a send of this service takes:
/// the structured send, the raw send naming its `Source`, and the raw send whose
/// sender is the message's own `From` line — "If you do not provide a value for
/// this parameter, you must specify a 'From' address in the raw text of the
/// message" (`SendRawEmailRequest$Source`).
///
/// `configuration_set`, when named, is carried by every form, since every send
/// takes the same `ConfigurationSetName`.
fn sends_from(source: &str, configuration_set: Option<&str>) -> Vec<Send> {
    const RECIPIENT: &str = "recipient@example.net";
    let raw = ::aws_smithy_types::base64::encode(raw_message(source, RECIPIENT));
    let mut rows = vec![
        Send {
            case: "SendEmail naming a Source",
            action: "SendEmail",
            params: vec![
                field("Source", source),
                field("Destination.ToAddresses.member.1", RECIPIENT),
                field("Message.Subject.Data", "Quarterly report"),
                field("Message.Body.Text.Data", "See attached."),
            ],
        },
        Send {
            case: "SendRawEmail naming a Source",
            action: "SendRawEmail",
            params: vec![
                field("RawMessage.Data", raw.clone()),
                field("Source", source),
                field("Destinations.member.1", RECIPIENT),
            ],
        },
        Send {
            case: "SendRawEmail reading the message's own From line",
            action: "SendRawEmail",
            params: vec![field("RawMessage.Data", raw)],
        },
    ];
    if let Some(name) = configuration_set {
        for row in &mut rows {
            row.params.push(field("ConfigurationSetName", name));
        }
    }
    rows
}

/// One identity row: how the account came to hold it (or not), the `Source` a
/// send then names under it, and the addr-spec a refusal reports.
struct Identity {
    /// What makes this row different from the others.
    case: &'static str,
    /// The verification request that records it, as `(action, parameters)`, and
    /// `None` for an identity nobody ever asked about.
    requested: Option<(&'static str, Vec<Field>)>,
    /// The `Source` (or `From` line) a send names.
    source: &'static str,
    /// The identity the gate checks, which is the addr-spec of that source.
    checked: &'static str,
    /// Whether the clock can ever carry this row's identity to a sender: an
    /// identity the account never asked about is not a record the clock owes
    /// anything.
    settles: bool,
}

/// The identity rows: the classes of record a `Source` can resolve to, plus the
/// absence of one.
fn identity_rows() -> Vec<Identity> {
    vec![
        Identity {
            case: "the mailbox itself, requested through VerifyEmailIdentity",
            requested: Some((
                "VerifyEmailIdentity",
                vec![field("EmailAddress", "probe@example.com")],
            )),
            source: "probe@example.com",
            checked: "probe@example.com",
            settles: true,
        },
        Identity {
            case: "the mailbox requested through the deprecated VerifyEmailAddress",
            requested: Some((
                "VerifyEmailAddress",
                vec![field("EmailAddress", "legacy@example.com")],
            )),
            source: "legacy@example.com",
            checked: "legacy@example.com",
            settles: true,
        },
        Identity {
            case: "a mailbox in a requested domain",
            requested: Some(("VerifyDomainIdentity", vec![field("Domain", "example.net")])),
            source: "sales@example.net",
            checked: "sales@example.net",
            settles: true,
        },
        Identity {
            case: "a mailbox under a subdomain of a requested domain",
            requested: Some(("VerifyDomainIdentity", vec![field("Domain", "example.org")])),
            source: "ops@mail.example.org",
            checked: "ops@mail.example.org",
            settles: true,
        },
        Identity {
            case: "the same mailbox written in display-name form",
            requested: Some((
                "VerifyEmailIdentity",
                vec![field("EmailAddress", "named@example.com")],
            )),
            source: "Pending Sender <named@example.com>",
            checked: "named@example.com",
            settles: true,
        },
        Identity {
            case: "an identity nobody ever asked about",
            requested: None,
            source: "stranger@example.com",
            checked: "stranger@example.com",
            settles: false,
        },
    ]
}

/// The `<Code>` of a refusal, and its `<Message>` with the XML escaping undone.
fn refusal(body: &str) -> (String, String) {
    (
        element(body, "Code").unwrap_or_default(),
        element(body, "Message")
            .map(|raw| xml_text(&raw))
            .unwrap_or_default(),
    )
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

/// `GetIdentityVerificationAttributes`, as the status it reports for `identity`,
/// or `None` when the account holds no such identity.
async fn reported_status(router: &MultiServiceRouter, identity: &str) -> Option<String> {
    let (status, body) = call(
        router,
        "GetIdentityVerificationAttributes",
        &[field("Identities.member.1", identity)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case attributes of {identity:?}: expected 200, got {status} with {body}"
    );
    map_entries(&body)
        .into_iter()
        .find(|(key, _)| key == identity)
        .and_then(|(_, value)| element(&value, "VerificationStatus"))
}

#[tokio::test]
async fn a_send_from_an_identity_whose_verification_was_only_requested_is_refused() {
    for row in identity_rows() {
        let (router, engine) = emulator_and_engine();
        // One other identity is carried all the way through, so every refusal
        // below is about the source the row names rather than about an account
        // with no identities at all.
        let (status, body) = call(
            &router,
            "VerifyEmailIdentity",
            &[field("EmailAddress", "already@example.com")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: VerifyEmailIdentity expected 200, got {status} with {body}",
            row.case
        );
        settle_verifications(&engine);

        if let Some((action, params)) = &row.requested {
            let (status, body) = call(&router, action, params).await;
            assert_eq!(
                status, 200,
                "case {:?}: {action} expected 200, got {status} with {body}",
                row.case
            );
            // Asking only STARTS the verification, which is what the read
            // reports while the span has not run out.
            if let Some(reported) = reported_status(&router, row.checked).await {
                assert_eq!(
                    reported, "Pending",
                    "case {:?}: expected the requested identity to read Pending, got {reported:?}",
                    row.case
                );
            }
        }

        for send in sends_from(row.source, None) {
            let (status, body) = call(&router, send.action, &send.params).await;
            assert_eq!(
                status, 400,
                "case {:?} through {}: expected HTTP 400, got {status} with {body}",
                row.case, send.case
            );
            let (code, stated) = refusal(&body);
            assert_eq!(
                code, "MessageRejected",
                "case {:?} through {}: expected the modelled MessageRejected, got {code:?} in \
                 {body}",
                row.case, send.case
            );
            let expected = expected_message(row.checked);
            assert_eq!(
                stated, expected,
                "case {:?} through {}: expected {expected:?}, got {stated:?}",
                row.case, send.case
            );
        }

        // A refused send never reaches the send log the quota is derived from.
        let sent = sent_last_24_hours(&router).await;
        assert_eq!(
            sent, 0.0,
            "case {:?}: expected an untouched send log after the refusals, got {sent}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_same_sends_are_accepted_once_the_verification_has_gone_through() {
    // The other half of the same rule: the gate is about the identity's
    // verification, so each row that HAS a record sends once the move the clock
    // owes it has been made — and the row with no record at all still does not.
    for row in identity_rows() {
        let (router, engine) = emulator_and_engine();
        if let Some((action, params)) = &row.requested {
            let (status, body) = call(&router, action, params).await;
            assert_eq!(
                status, 200,
                "case {:?}: {action} expected 200, got {status} with {body}",
                row.case
            );
        }
        settle_verifications(&engine);

        let expected_status = if row.settles { 200 } else { 400 };
        for send in sends_from(row.source, None) {
            let (status, body) = call(&router, send.action, &send.params).await;
            assert_eq!(
                status, expected_status,
                "case {:?} through {} past the span: expected {expected_status}, got {status} \
                 with {body}",
                row.case, send.case
            );
            if row.settles {
                assert!(
                    element(&body, "MessageId").is_some(),
                    "case {:?} through {}: an accepted send reports the id it stored, got {body}",
                    row.case,
                    send.case
                );
            }
        }
        let sent = sent_last_24_hours(&router).await;
        let expected_sent = if row.settles { 3.0 } else { 0.0 };
        assert_eq!(
            sent, expected_sent,
            "case {:?}: expected {expected_sent} counted recipients, got {sent}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_configuration_set_the_send_names_is_judged_ahead_of_the_identity() {
    // Both gates refuse the same send, each with its own modelled error, and the
    // set the request names is read FIRST: a send naming a set the account does
    // not hold is refused for the set, whether or not its identity would have
    // passed. Neither refusal creates the set and neither reaches the send log.
    let (router, engine) = emulator_and_engine();
    let (status, body) = call(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "probe@example.com")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case VerifyEmailIdentity: expected 200, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", "real-set")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateConfigurationSet: expected 200, got {status} with {body}"
    );

    // While the identity is pending: the unknown set is reported, the known set
    // leaves the identity gate to answer.
    let rows: &[(&str, Option<&str>, &str, String)] = &[
        (
            "a set the account does not hold",
            Some("no-such-set"),
            "ConfigurationSetDoesNotExist",
            "Configuration set <no-such-set> does not exist.".to_string(),
        ),
        (
            "a set the account holds",
            Some("real-set"),
            "MessageRejected",
            expected_message("probe@example.com"),
        ),
        (
            "no set at all",
            None,
            "MessageRejected",
            expected_message("probe@example.com"),
        ),
    ];
    for (case, configuration_set, expected_code, expected_message) in rows {
        for send in sends_from("probe@example.com", *configuration_set) {
            let (status, body) = call(&router, send.action, &send.params).await;
            assert_eq!(
                status, 400,
                "case {case} through {}: expected HTTP 400, got {status} with {body}",
                send.case
            );
            let (code, stated) = refusal(&body);
            assert_eq!(
                (code.as_str(), stated.as_str()),
                (*expected_code, expected_message.as_str()),
                "case {case} through {}: expected {expected_code} / {expected_message:?}, got \
                 {code:?} / {stated:?}",
                send.case
            );
        }
    }

    // The refused sends created nothing and logged nothing.
    let (status, body) = call(
        &router,
        "DescribeConfigurationSet",
        &[field("ConfigurationSetName", "no-such-set")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case describe no-such-set: a refused send must not create the set it named, got \
         {status} with {body}"
    );
    let sent = sent_last_24_hours(&router).await;
    assert_eq!(
        sent, 0.0,
        "case after the refusals: expected an untouched send log, got {sent}"
    );

    // And once the verification has gone through, the send naming the set the
    // account holds is accepted: the set gate above was about the set.
    settle_verifications(&engine);
    for send in sends_from("probe@example.com", Some("real-set")) {
        let (status, body) = call(&router, send.action, &send.params).await;
        assert_eq!(
            status, 200,
            "case a held set past the span through {}: expected 200, got {status} with {body}",
            send.case
        );
    }
}

#[tokio::test]
async fn the_move_out_of_pending_is_owed_by_the_clock() {
    // The span is not a test fixture: nothing but time moves the record, so this
    // case drives the real clock once. A caller reading or sending straight after
    // asking for the verification sees the pending identity and its refusal; past
    // the span, the same reads and the same send answer the other way.
    let (router, _engine) = emulator_and_engine();
    let (status, body) = call(
        &router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", "clock@example.com")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case VerifyEmailIdentity: expected 200, got {status} with {body}"
    );

    let reported = reported_status(&router, "clock@example.com").await;
    assert_eq!(
        reported.as_deref(),
        Some("Pending"),
        "case inside the span: expected Pending, got {reported:?}"
    );
    for send in sends_from("clock@example.com", None) {
        let (status, body) = call(&router, send.action, &send.params).await;
        assert_eq!(
            status, 400,
            "case inside the span through {}: expected HTTP 400, got {status} with {body}",
            send.case
        );
    }

    // Past the span the clock has made the move due, whoever asks next.
    tokio::time::sleep(IDENTITY_VERIFICATION_SPAN + std::time::Duration::from_millis(250)).await;
    let reported = reported_status(&router, "clock@example.com").await;
    assert_eq!(
        reported.as_deref(),
        Some("Success"),
        "case past the span: expected Success, got {reported:?}"
    );
    // And a settled identity stays settled, so the next reader sees the same.
    let again = reported_status(&router, "clock@example.com").await;
    assert_eq!(
        again, reported,
        "case read twice past the span: expected {reported:?}, got {again:?}"
    );
    for send in sends_from("clock@example.com", None) {
        let (status, body) = call(&router, send.action, &send.params).await;
        assert_eq!(
            status, 200,
            "case past the span through {}: expected 200, got {status} with {body}",
            send.case
        );
    }
}
