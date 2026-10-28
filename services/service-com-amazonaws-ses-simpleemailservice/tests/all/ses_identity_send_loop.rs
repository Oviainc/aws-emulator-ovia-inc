// SPDX-License-Identifier: CC0-1.0

//! The SES identity + send loop, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so each
//! call is a real awsQuery request against the service: form-urlencoded body in,
//! XML out, one shared engine behind both. What is asserted is the RULE each
//! operation follows over a SET of inputs, not one recorded request:
//!
//! * an identity is recorded once per name, whatever the form of the name, and
//!   the two read operations report exactly what was recorded;
//! * `ListIdentities` returns the stored identities in one stable order,
//!   filtered by the model's own `IdentityType` value set, and paging with
//!   `MaxItems`/`NextToken` visits every identity exactly once for EVERY page
//!   size;
//! * a send is accepted only from a source SES would accept, and every
//!   recipient of an accepted send counts against the 24-hour quota;
//! * a refusal carries the modeled error code and HTTP 400, and leaves the send
//!   log alone.

use crate::harness::{
    call, element, elements, emulator, emulator_and_engine, field, map_entries,
    settle_verifications, Field,
};
use service_com_amazonaws_ses_simpleemailservice::ops::IDENTITY_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

/// `VerifyEmailIdentity`.
async fn verify_email(router: &MultiServiceRouter, address: &str) -> (u16, String) {
    call(
        router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", address)],
    )
    .await
}

/// `VerifyDomainIdentity`.
async fn verify_domain(router: &MultiServiceRouter, domain: &str) -> (u16, String) {
    call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await
}

/// `ListIdentities`, as `(the identities on this page, the next token)`.
async fn list_identities(
    router: &MultiServiceRouter,
    identity_type: Option<&str>,
    max_items: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut params: Vec<Field> = Vec::new();
    if let Some(identity_type) = identity_type {
        params.push(field("IdentityType", identity_type));
    }
    if let Some(max_items) = max_items {
        params.push(field("MaxItems", max_items.to_string()));
    }
    if let Some(next_token) = next_token {
        params.push(field("NextToken", next_token));
    }
    let (status, body) = call(router, "ListIdentities", &params).await;
    assert_eq!(
        status, 200,
        "ListIdentities(type={identity_type:?}, max={max_items:?}, token={next_token:?}): \
         expected 200, got {status} with {body}"
    );
    let identities = element(&body, "Identities")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default();
    (identities, element(&body, "NextToken"))
}

/// `GetIdentityVerificationAttributes`, as the map it reports.
async fn verification_attributes(
    router: &MultiServiceRouter,
    identities: &[&str],
) -> Vec<(String, String)> {
    let params: Vec<Field> = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| field(format!("Identities.member.{}", index + 1), *identity))
        .collect();
    let (status, body) = call(router, "GetIdentityVerificationAttributes", &params).await;
    assert_eq!(
        status, 200,
        "GetIdentityVerificationAttributes({identities:?}): expected 200, got {status} with {body}"
    );
    map_entries(&body)
}

/// A `SendEmail` request: a source, the three recipient lists, and an optional
/// configuration set.
struct SendRequest<'a> {
    source: &'a str,
    to: &'a [&'a str],
    cc: &'a [&'a str],
    bcc: &'a [&'a str],
    configuration_set: Option<&'a str>,
}

impl<'a> SendRequest<'a> {
    /// The common case: one source, To: recipients only.
    fn simple(source: &'a str, to: &'a [&'a str]) -> Self {
        Self {
            source,
            to,
            cc: &[],
            bcc: &[],
            configuration_set: None,
        }
    }

    fn params(&self) -> Vec<Field> {
        let mut params = vec![
            field("Source", self.source),
            field("Message.Subject.Data", "Quarterly report"),
            field("Message.Body.Text.Data", "See attached."),
        ];
        for (name, addresses) in [
            ("ToAddresses", self.to),
            ("CcAddresses", self.cc),
            ("BccAddresses", self.bcc),
        ] {
            if addresses.is_empty() {
                // `Destination` is `@required`, so it must be on the wire even
                // when it names nobody: the bare list key carries the member with
                // no elements, which is how a caller asks SES to send to nobody.
                params.push(field(format!("Destination.{name}"), ""));
                continue;
            }
            for (index, address) in addresses.iter().enumerate() {
                params.push(field(
                    format!("Destination.{name}.member.{}", index + 1),
                    *address,
                ));
            }
        }
        if let Some(configuration_set) = self.configuration_set {
            params.push(field("ConfigurationSetName", configuration_set));
        }
        params
    }
}

/// `SendEmail`.
async fn send_email(router: &MultiServiceRouter, message: &SendRequest<'_>) -> (u16, String) {
    call(router, "SendEmail", &message.params()).await
}

/// `GetSendQuota`, as `(Max24HourSend, MaxSendRate, SentLast24Hours)`.
async fn send_quota(router: &MultiServiceRouter) -> (f64, f64, f64) {
    let (status, body) = call(router, "GetSendQuota", &[]).await;
    assert_eq!(
        status, 200,
        "GetSendQuota: expected 200, got {status} with {body}"
    );
    let number = |tag: &str| -> f64 {
        let text = element(&body, tag)
            .unwrap_or_else(|| panic!("GetSendQuota must report {tag}, got {body}"));
        assert!(
            !text.starts_with('-'),
            "GetSendQuota's {tag} must not be negative, got {text:?}"
        );
        text.parse::<f64>()
            .unwrap_or_else(|_| panic!("GetSendQuota's {tag} must be a number, got {text:?}"))
    };
    (
        number("Max24HourSend"),
        number("MaxSendRate"),
        number("SentLast24Hours"),
    )
}

/// Addresses spanning the forms an email identity can take: a plain mailbox, a
/// dotted local part with a `+` tag, a subdomain under a multi-label public
/// suffix, and the shortest labels that are still an address. The rule is about
/// the identity STORE, so a row's only job is to be a different name.
const EMAIL_IDENTITIES: &[&str] = &[
    "sender@example.com",
    "first.last+quarterly@example.com",
    "ops@mail.corp.example.co.uk",
    "a@e.io",
];

#[tokio::test]
async fn the_identity_and_send_loop_is_stateful_end_to_end() {
    let (router, engine) = emulator_and_engine();

    // Nothing is verified yet, so the list is empty and nothing has been sent:
    // the reads report state, never a canned answer.
    let (identities, next) = list_identities(&router, None, None, None).await;
    assert!(
        identities.is_empty() && next.is_none(),
        "case fresh emulator: expected no identities and no token, got {identities:?} / {next:?}"
    );
    let (max_24_hour, max_rate, sent) = send_quota(&router).await;
    assert_eq!(
        (max_24_hour, max_rate, sent),
        (200.0, 1.0, 0.0),
        "case fresh emulator: expected the sandbox quota with nothing sent, got \
         ({max_24_hour}, {max_rate}, {sent})"
    );

    // VerifyEmailIdentity records each address, and doing it twice records it
    // once: re-verification is idempotent.
    for address in EMAIL_IDENTITIES {
        for attempt in 1..=2 {
            let (status, body) = verify_email(&router, address).await;
            assert_eq!(
                status, 200,
                "case verify {address:?} attempt {attempt}: expected 200, got {status} with {body}"
            );
        }
    }

    let mut expected: Vec<String> = EMAIL_IDENTITIES.iter().map(|a| (*a).to_string()).collect();
    expected.sort();
    let (identities, next) = list_identities(&router, None, None, None).await;
    assert_eq!(
        identities, expected,
        "case after verifying {EMAIL_IDENTITIES:?}: expected exactly those identities once each, \
         in stable order, got {identities:?}"
    );
    assert!(
        next.is_none(),
        "case unpaged listing: expected no NextToken, got {next:?}"
    );

    // Every verified address reports a status, and an email-address identity has
    // NO verification token ("Null for email address identities"). An address
    // that was never verified is absent from the map rather than reported with an
    // invented status.
    let mut asked: Vec<&str> = EMAIL_IDENTITIES.to_vec();
    asked.push("never-verified@example.net");
    let attributes = verification_attributes(&router, &asked).await;
    assert_eq!(
        attributes.len(),
        EMAIL_IDENTITIES.len(),
        "case {asked:?}: expected an entry only for the verified identities, got {attributes:?}"
    );
    for (identity, value) in &attributes {
        // The request above only ASKED for the verification: nobody has followed
        // the link in the message yet, so the identity is pending.
        assert_eq!(
            element(value, "VerificationStatus").as_deref(),
            Some("Pending"),
            "case {identity:?}: expected a pending identity, got {value:?}"
        );
        assert!(
            element(value, "VerificationToken").is_none(),
            "case {identity:?}: an email-address identity carries no VerificationToken, got \
             {value:?}"
        );
    }
    assert!(
        !attributes
            .iter()
            .any(|(identity, _)| identity == "never-verified@example.net"),
        "case never-verified@example.net: expected no entry at all, got {attributes:?}"
    );

    // Once the move the clock owes each record falls due, the same read reports
    // it verified — and only then is the address a sender.
    settle_verifications(&engine);
    let attributes = verification_attributes(&router, EMAIL_IDENTITIES).await;
    for (identity, value) in &attributes {
        assert_eq!(
            element(value, "VerificationStatus").as_deref(),
            Some("Success"),
            "case {identity:?} past the span: expected a verified identity, got {value:?}"
        );
    }

    // A send from each verified address is accepted, logged, and answered with a
    // fresh id.
    let mut message_ids: Vec<String> = Vec::new();
    for address in EMAIL_IDENTITIES {
        let (status, body) =
            send_email(&router, &SendRequest::simple(address, &["recipient@example.net"])).await;
        assert_eq!(
            status, 200,
            "case send from {address:?}: expected 200, got {status} with {body}"
        );
        let message_id = element(&body, "MessageId")
            .unwrap_or_else(|| panic!("case send from {address:?}: expected a MessageId: {body}"));
        assert!(
            message_id.starts_with("0100") && message_id.ends_with("-000000"),
            "case send from {address:?}: expected an SES-shaped MessageId, got {message_id:?}"
        );
        assert!(
            !message_ids.contains(&message_id),
            "case send from {address:?}: expected a fresh MessageId, got the earlier \
             {message_id:?} again"
        );
        message_ids.push(message_id);
    }

    // The quota reader computes the usage from that log.
    let (max_24_hour, max_rate, sent) = send_quota(&router).await;
    assert_eq!(
        (max_24_hour, max_rate, sent),
        (200.0, 1.0, EMAIL_IDENTITIES.len() as f64),
        "case after {} single-recipient sends: expected the sandbox quota with one recipient \
         counted per send, got ({max_24_hour}, {max_rate}, {sent})",
        EMAIL_IDENTITIES.len()
    );

    // DeleteIdentity removes the identity, so both readers stop reporting it, and
    // deleting one that is not there still succeeds.
    for identity in EMAIL_IDENTITIES.iter().chain(["ghost@example.net"].iter()) {
        let (status, body) = call(&router, "DeleteIdentity", &[field("Identity", *identity)]).await;
        assert_eq!(
            status, 200,
            "case delete {identity:?}: expected 200, got {status} with {body}"
        );
    }
    let (identities, _) = list_identities(&router, None, None, None).await;
    assert!(
        identities.is_empty(),
        "case after deleting every identity: expected an empty list, got {identities:?}"
    );
    let attributes = verification_attributes(&router, EMAIL_IDENTITIES).await;
    assert!(
        attributes.is_empty(),
        "case after deleting every identity: expected no verification attributes, got \
         {attributes:?}"
    );

    // The send log is NOT touched by deleting the sender: the quota still reports
    // what was sent.
    let (_, _, sent) = send_quota(&router).await;
    assert_eq!(
        sent,
        EMAIL_IDENTITIES.len() as f64,
        "case after deleting the senders: the send log is unchanged, expected {}, got {sent}",
        EMAIL_IDENTITIES.len()
    );
}

#[tokio::test]
async fn list_identities_filters_by_type_and_pages_over_every_page_size() {
    let router = emulator();
    let domains = ["beta.example.com", "alpha.example.org"];
    let emails = ["zoe@example.com", "mia@example.com", "ada@example.com"];
    for domain in domains {
        let (status, body) = verify_domain(&router, domain).await;
        assert_eq!(
            status, 200,
            "case verify domain {domain:?}: expected 200, got {status} with {body}"
        );
    }
    for address in emails {
        let (status, body) = verify_email(&router, address).await;
        assert_eq!(
            status, 200,
            "case verify address {address:?}: expected 200, got {status} with {body}"
        );
    }

    let sorted = |names: &[&str]| {
        let mut names: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
        names.sort();
        names
    };
    let all = sorted(&[domains.as_slice(), emails.as_slice()].concat());

    // The filter's value set is the model's own `IdentityType` enum, not a list
    // written here: `IDENTITY_TYPE_VALUES` is generated from the model.
    for identity_type in IDENTITY_TYPE_VALUES {
        let expected = match *identity_type {
            "Domain" => sorted(&domains),
            "EmailAddress" => sorted(&emails),
            other => panic!("unhandled IdentityType {other:?} from the model's value set"),
        };
        let (listed, _) = list_identities(&router, Some(identity_type), None, None).await;
        assert_eq!(
            listed, expected,
            "case IdentityType={identity_type:?}: expected only that class, got {listed:?}"
        );
    }

    // Paging is an invariant, so it is checked for EVERY page size from one to
    // one past the identity count, plus the published upper bound: following
    // NextToken must visit every identity exactly once, in the order the unpaged
    // listing uses.
    for page_size in (1..=(all.len() as i64 + 1)).chain([1000]) {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (page, next) =
                list_identities(&router, None, Some(page_size), token.as_deref()).await;
            assert!(
                page.len() as i64 <= page_size,
                "case MaxItems={page_size} page {pages}: expected at most {page_size} identities, \
                 got {page:?}"
            );
            pages += 1;
            seen.extend(page);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= all.len() + 1,
                "case MaxItems={page_size}: pagination did not terminate, saw {seen:?}"
            );
        }
        assert_eq!(
            seen, all,
            "case MaxItems={page_size}: expected every identity exactly once in stable order, \
             got {seen:?}"
        );
    }

    // Paging under a filter resumes within that filter only.
    let (page, token) = list_identities(&router, Some("EmailAddress"), Some(1), None).await;
    assert_eq!(
        page,
        sorted(&emails)[..1],
        "case IdentityType=EmailAddress MaxItems=1: expected the first address, got {page:?}"
    );
    let (page, _) =
        list_identities(&router, Some("EmailAddress"), Some(10), token.as_deref()).await;
    assert_eq!(
        page,
        sorted(&emails)[1..],
        "case IdentityType=EmailAddress resumed: expected the remaining addresses, got {page:?}"
    );

    // MaxItems' published range is "1-1000 inclusive": both bounds are accepted
    // and one past each is refused.
    for max_items in [1, 1000] {
        let (status, _) = call(
            &router,
            "ListIdentities",
            &[field("MaxItems", max_items.to_string())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxItems={max_items}: a published bound is accepted, got {status}"
        );
    }
    for max_items in [0, 1001, -1] {
        let (status, body) = call(
            &router,
            "ListIdentities",
            &[field("MaxItems", max_items.to_string())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxItems={max_items}: outside 1-1000 is refused, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("ValidationException"),
            "case MaxItems={max_items}: expected a validation refusal, got {body}"
        );
    }
}

#[tokio::test]
async fn a_domain_identity_carries_a_stable_token_and_covers_its_mailboxes() {
    let (router, engine) = emulator_and_engine();
    let (status, body) = verify_domain(&router, "example.org").await;
    assert_eq!(
        status, 200,
        "case verify example.org: expected 200, got {status} with {body}"
    );
    let token = element(&body, "VerificationToken")
        .unwrap_or_else(|| panic!("case verify example.org: expected a token, got {body}"));
    assert!(
        token.len() >= 32,
        "case verify example.org: expected an opaque token, got {token:?}"
    );

    // Re-verifying is idempotent and the token is STORED, so it is stable across
    // reads rather than minted per call.
    let (_, body) = verify_domain(&router, "example.org").await;
    assert_eq!(
        element(&body, "VerificationToken").as_deref(),
        Some(token.as_str()),
        "case re-verify example.org: expected the stored token {token:?}, got {body}"
    );
    let attributes = verification_attributes(&router, &["example.org"]).await;
    let (_, value) = attributes
        .first()
        .unwrap_or_else(|| panic!("case example.org: expected one entry, got {attributes:?}"));
    assert_eq!(
        element(value, "VerificationToken").as_deref(),
        Some(token.as_str()),
        "case example.org: the reported token must be the stored one {token:?}, got {value:?}"
    );

    // "The message must be sent from a verified email address or domain": a
    // verified domain covers the mailboxes in it and under its subdomains, and
    // nothing else. The request above only asked for the verification, so the
    // store is settled past the span the clock owes that record first.
    settle_verifications(&engine);
    let cases: &[(&str, u16)] = &[
        ("anyone@example.org", 200),
        ("anyone@mail.example.org", 200),
        ("anyone@deep.mail.example.org", 200),
        ("anyone@notexample.org", 400),
        ("anyone@example.org.evil.com", 400),
        ("example.org@example.net", 400),
    ];
    for (source, expected) in cases {
        let (status, body) =
            send_email(&router, &SendRequest::simple(source, &["recipient@example.net"])).await;
        assert_eq!(
            status, *expected,
            "case send from {source:?} under a verified example.org: expected {expected}, got \
             {status} with {body}"
        );
    }

    // And a domain identity is listed as a Domain, never as an EmailAddress.
    let (listed, _) = list_identities(&router, Some("Domain"), None, None).await;
    assert_eq!(
        listed,
        vec!["example.org".to_string()],
        "case IdentityType=Domain: expected the verified domain, got {listed:?}"
    );
    let (listed, _) = list_identities(&router, Some("EmailAddress"), None, None).await;
    assert!(
        listed.is_empty(),
        "case IdentityType=EmailAddress: expected none, got {listed:?}"
    );
}

#[tokio::test]
async fn the_quota_counts_every_recipient_of_every_send() {
    let (router, engine) = emulator_and_engine();
    verify_email(&router, "sender@example.com").await;
    settle_verifications(&engine);

    // Each row differs in HOW the recipients are spread across To:/CC:/BCC:; the
    // rule is that every one of them counts once, because SES counts "the total
    // number of recipients (including each recipient in the To:, CC: and BCC:
    // fields)" against the 24-hour quota.
    /// One row: the case name, then the To:, CC: and BCC: recipients.
    type Row<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a [&'a str]);
    let rows: &[Row<'_>] = &[
        ("one To:", &["a@example.net"], &[], &[]),
        ("two To:", &["a@example.net", "b@example.net"], &[], &[]),
        ("To: and CC:", &["a@example.net"], &["c@example.net"], &[]),
        (
            "To:, CC: and BCC:",
            &["a@example.net"],
            &["c@example.net"],
            &["d@example.net", "e@example.net"],
        ),
    ];
    let mut expected = 0.0;
    let mut ids: Vec<String> = Vec::new();
    for (case, to, cc, bcc) in rows {
        let (status, body) = send_email(
            &router,
            &SendRequest {
                source: "sender@example.com",
                to,
                cc,
                bcc,
                configuration_set: None,
            },
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let id = element(&body, "MessageId")
            .unwrap_or_else(|| panic!("case {case}: expected a MessageId, got {body}"));
        assert!(
            !ids.contains(&id),
            "case {case}: expected a fresh MessageId, got the earlier {id:?} again"
        );
        ids.push(id);

        expected += (to.len() + cc.len() + bcc.len()) as f64;
        let (_, _, sent) = send_quota(&router).await;
        assert_eq!(
            sent, expected,
            "case {case}: expected SentLast24Hours to count every recipient ({expected}), got \
             {sent}"
        );
    }
}

#[tokio::test]
async fn a_send_is_refused_with_the_modeled_error_and_leaves_the_log_alone() {
    let (router, engine) = emulator_and_engine();
    verify_email(&router, "sender@example.com").await;
    settle_verifications(&engine);

    let many: Vec<String> = (0..51).map(|n| format!("r{n}@example.net")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let rows: Vec<(&str, SendRequest<'_>, &str)> = vec![
        (
            "an unverified source",
            SendRequest::simple("stranger@example.com", &["a@example.net"]),
            "MessageRejected",
        ),
        (
            "a source whose domain is not verified either",
            SendRequest::simple("sender@example.net", &["a@example.net"]),
            "MessageRejected",
        ),
        (
            "no recipient at all",
            SendRequest::simple("sender@example.com", &[]),
            "MessageRejected",
        ),
        (
            "one recipient past the 50-recipient ceiling",
            SendRequest::simple("sender@example.com", &many),
            "MessageRejected",
        ),
        (
            "a recipient that is not an address",
            SendRequest::simple("sender@example.com", &["not-an-address"]),
            "MessageRejected",
        ),
        (
            "a recipient with no top-level domain",
            SendRequest::simple("sender@example.com", &["user@localhost"]),
            "MessageRejected",
        ),
        (
            "one invalid recipient among valid ones",
            SendRequest::simple("sender@example.com", &["a@example.net", "b@@example.net"]),
            "MessageRejected",
        ),
        (
            "a configuration set that does not exist",
            SendRequest {
                source: "sender@example.com",
                to: &["a@example.net"],
                cc: &[],
                bcc: &[],
                configuration_set: Some("no-such-set"),
            },
            "ConfigurationSetDoesNotExist",
        ),
    ];

    for (case, message, expected_code) in &rows {
        let (status, body) = send_email(&router, message).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some(*expected_code),
            "case {case}: expected the modeled {expected_code}, got {body}"
        );
    }

    // Exactly 50 recipients is inside the published ceiling, so it is accepted.
    let fifty: Vec<String> = (0..50).map(|n| format!("r{n}@example.net")).collect();
    let fifty: Vec<&str> = fifty.iter().map(String::as_str).collect();
    let (status, body) = send_email(&router, &SendRequest::simple("sender@example.com", &fifty)).await;
    assert_eq!(
        status, 200,
        "case exactly 50 recipients: expected 200, got {status} with {body}"
    );

    // None of the refusals reached the send log: only the accepted send did.
    let (_, _, sent) = send_quota(&router).await;
    assert_eq!(
        sent, 50.0,
        "case after {} refusals and one accepted 50-recipient send: expected 50 counted \
         recipients, got {sent}",
        rows.len()
    );
}

#[tokio::test]
async fn a_listing_with_no_max_items_is_still_one_bounded_page() {
    let router = emulator();
    // More identities than the default page size, so the default is observable:
    // a request that names no MaxItems gets one page plus a token, never the
    // whole store in one response.
    const DEFAULT_PAGE: usize = 100;
    let addresses: Vec<String> = (0..DEFAULT_PAGE + 5)
        .map(|n| format!("user{n:04}@example.com"))
        .collect();
    for address in &addresses {
        let (status, body) = verify_email(&router, address).await;
        assert_eq!(
            status, 200,
            "case verify {address:?}: expected 200, got {status} with {body}"
        );
    }
    let mut expected = addresses.clone();
    expected.sort();

    let (page, token) = list_identities(&router, None, None, None).await;
    assert_eq!(
        page.len(),
        DEFAULT_PAGE,
        "case {} identities, no MaxItems: expected the default page size {DEFAULT_PAGE}, got {}",
        addresses.len(),
        page.len()
    );
    assert_eq!(
        page, expected[..DEFAULT_PAGE],
        "case no MaxItems: expected the first page in stable order, got {page:?}"
    );
    let token = token.unwrap_or_else(|| {
        panic!("case {} identities, no MaxItems: expected a NextToken", addresses.len())
    });
    let (rest, next) = list_identities(&router, None, None, Some(&token)).await;
    assert_eq!(
        rest,
        expected[DEFAULT_PAGE..],
        "case resumed with the default page size: expected the remaining identities, got {rest:?}"
    );
    assert!(
        next.is_none(),
        "case resumed to the end: expected no NextToken, got {next:?}"
    );
}

#[tokio::test]
async fn a_send_past_the_accounts_24_hour_quota_is_refused() {
    let (router, engine) = emulator_and_engine();
    verify_email(&router, "sender@example.com").await;
    settle_verifications(&engine);
    let (quota, _, _) = send_quota(&router).await;

    // Fill the quota exactly, in the largest messages a send may carry (50
    // recipients each), then ask for one recipient more: every recipient counts,
    // so the next send is refused rather than accepted past the stored quota.
    let batch: Vec<String> = (0..50).map(|n| format!("r{n}@example.net")).collect();
    let batch: Vec<&str> = batch.iter().map(String::as_str).collect();
    let sends = (quota as usize) / batch.len();
    for round in 0..sends {
        let (status, body) = send_email(&router, &SendRequest::simple("sender@example.com", &batch)).await;
        assert_eq!(
            status, 200,
            "case send {} of {sends} inside the quota: expected 200, got {status} with {body}",
            round + 1
        );
    }
    let (_, _, sent) = send_quota(&router).await;
    assert_eq!(
        sent, quota,
        "case quota filled exactly: expected SentLast24Hours to reach {quota}, got {sent}"
    );

    let (status, body) =
        send_email(&router, &SendRequest::simple("sender@example.com", &["one-more@example.net"])).await;
    assert_eq!(
        status, 400,
        "case one recipient past the {quota}-message quota: expected 400, got {status} with {body}"
    );
    assert_eq!(
        element(&body, "Code").as_deref(),
        Some("MessageRejected"),
        "case one recipient past the quota: expected the modeled MessageRejected, got {body}"
    );
    let (_, _, sent) = send_quota(&router).await;
    assert_eq!(
        sent, quota,
        "case after the refusal: the send log is unchanged, expected {quota}, got {sent}"
    );
}

#[tokio::test]
async fn an_identity_that_is_not_an_address_or_domain_is_refused() {
    let router = emulator();

    // `EmailAddress` and `Domain` are plain strings in the model, so the boundary
    // validator admits these; SES refuses them rather than recording an identity
    // nothing could send from.
    let bad_addresses = [
        "",
        "not-an-address",
        "user@localhost",
        "@example.com",
        "user@",
        "user@@example.com",
        "user name@example.com",
        "user@example..com",
        "üser@example.com",
        // The local part is a dot-atom, so no atom of it may be empty: a leading,
        // trailing or doubled dot is not an address.
        ".user@example.com",
        "user.@example.com",
        "user..name@example.com",
        ".@example.com",
    ];
    for address in bad_addresses {
        let (status, body) = verify_email(&router, address).await;
        assert_eq!(
            status, 400,
            "case VerifyEmailIdentity({address:?}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("ValidationException"),
            "case VerifyEmailIdentity({address:?}): expected a validation refusal, got {body}"
        );
    }
    let bad_domains = ["", "localhost", "example..com", "-", "example.1"];
    for domain in bad_domains {
        let (status, body) = verify_domain(&router, domain).await;
        assert_eq!(
            status, 400,
            "case VerifyDomainIdentity({domain:?}): expected 400, got {status} with {body}"
        );
    }

    // Nothing was recorded by any of them.
    let (identities, _) = list_identities(&router, None, None, None).await;
    assert!(
        identities.is_empty(),
        "case after only refused verifications: expected no identities, got {identities:?}"
    );

    // The forms that ARE addresses are recorded, and what is recorded is the
    // mailbox: an address given in RFC 5322 display-name form is stored by its
    // addr-spec, never by the display text.
    for address in [
        "user@example.com",
        "\"Ops Team\" <ops@example.com>",
        // A dot-atom local part with every atom non-empty, and the atom
        // characters RFC 5322 allows beside the alphanumerics.
        "first.last@mail.example.com",
        "a+b_c-d{e}@example.com",
    ] {
        let (status, body) = verify_email(&router, address).await;
        assert_eq!(
            status, 200,
            "case VerifyEmailIdentity({address:?}): expected 200, got {status} with {body}"
        );
    }
    let (identities, _) = list_identities(&router, None, None, None).await;
    assert_eq!(
        identities,
        vec![
            "a+b_c-d{e}@example.com".to_string(),
            "first.last@mail.example.com".to_string(),
            "ops@example.com".to_string(),
            "user@example.com".to_string()
        ],
        "case display-name and bare address: expected the addr-spec of each, got {identities:?}"
    );
}
