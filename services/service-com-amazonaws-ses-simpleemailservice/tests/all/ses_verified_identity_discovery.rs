// SPDX-License-Identifier: CC0-1.0

//! Finding the account's SENDING identities through the v1 identity plane, end to end
//! over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind them
//! all.
//!
//! One rule, asserted over a SET of identities rather than the one a report cited: an
//! identity the account has verified is enumerated by `ListIdentities` under its OWN
//! `IdentityType` and reported by `GetIdentityVerificationAttributes` with
//! `VerificationStatus` `Success` — carrying the stored TXT `VerificationToken` for a
//! domain identity and none for an address identity — so the pre-flight every SES
//! client performs (page the listing per identity class, then read the verification of
//! what it listed) resolves a sender, and a `FromEmailAddress` that identity covers is
//! then admitted by `CreateCustomVerificationEmailTemplate` and echoed back by
//! `GetCustomVerificationEmailTemplate`. An identity whose verification has not come
//! through yet is listed all the same, and reported in the state it is actually in, so
//! the pre-flight picks it up as no sender.
//!
//! The rows differ along what the rule generalises over: the class of identity (a
//! mailbox, an apex domain, a subdomain), the form of the name (a plus-tagged local
//! part, a mixed-case domain, a two-label public suffix) and how many identities the
//! account holds of each class — and the discovery walk itself pages the listing, so
//! the count is not confined to one page.

use crate::harness::{
    call, element, elements, emulator_and_engine, error_envelope, field, map_entries,
    settle_verifications, xml_text, Field,
};
use service_com_amazonaws_ses_simpleemailservice::ops::IDENTITY_TYPE_VALUES;
use service_com_amazonaws_ses_simpleemailservice::state::MAX_IDENTITIES_PER_CALL;
use smithy_server_runtime::MultiServiceRouter;

/// The state SES reports for an identity it accepts as a sender
/// (`IdentityVerificationAttributes$VerificationStatus`, whose value set
/// `smithy_query(query="shape",shape="VerificationStatus")` declares as
/// `Pending, Success, Failed, TemporaryFailure, NotStarted`).
const SUCCESS: &str = "Success";

/// The state SES reports for an identity whose verification has been requested and has
/// not come through: the owner has yet to follow the link in the message, or to publish
/// the domain's TXT record.
const PENDING: &str = "Pending";

/// The decoded length of a domain identity's TXT verification token, as the account's
/// shared identity registry mints it (`service_common::ses_identities`).
const VERIFICATION_TOKEN_BYTES: usize = 32;

/// The code SES answers a sender it will not send a custom verification email from.
const NOT_VERIFIED: &str = "FromEmailAddressNotVerified";

/// The page size the discovery walk asks for, small enough that the account's
/// identities of one class do not fit on a single page: the walk has to follow
/// `NextToken` to see them all, which is what a client paging the listing does.
const PAGE_SIZE: usize = 2;

/// One row: an identity the account verifies, and a `FromEmailAddress` it covers.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The identity as the verification request spells it.
    identity: &'static str,
    /// The `IdentityType` the listing must file it under.
    identity_type: &'static str,
    /// A sender that identity covers: the mailbox itself for an address identity, a
    /// mailbox under the domain for a domain identity.
    sender: &'static str,
}

/// The rows, spanning the identity classes and the forms their names take.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a mailbox identity",
            identity: "sender@example.com",
            identity_type: "EmailAddress",
            sender: "sender@example.com",
        },
        Row {
            case: "a mailbox identity with a plus-tagged local part",
            identity: "ops+ses@example.co.uk",
            identity_type: "EmailAddress",
            sender: "ops+ses@example.co.uk",
        },
        Row {
            case: "a mailbox identity at a third-level domain",
            identity: "postmaster@mail.example.name",
            identity_type: "EmailAddress",
            sender: "postmaster@mail.example.name",
        },
        Row {
            case: "an apex domain identity",
            identity: "example.net",
            identity_type: "Domain",
            sender: "noreply@example.net",
        },
        Row {
            case: "a subdomain identity",
            identity: "mail.example.org",
            identity_type: "Domain",
            sender: "bounce@mail.example.org",
        },
        Row {
            case: "a mixed-case domain identity",
            identity: "MiXeD.Example.IO",
            identity_type: "Domain",
            sender: "noreply@MiXeD.Example.IO",
        },
    ]
}

/// Ask SES to verify `identity`, on the operation its class is recorded with.
async fn request_verification(router: &MultiServiceRouter, case: &str, identity: &str) {
    let (action, member) = if identity.contains('@') {
        ("VerifyEmailIdentity", "EmailAddress")
    } else {
        ("VerifyDomainIdentity", "Domain")
    };
    let (status, body) = call(router, action, &[field(member, identity)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action}({identity:?}) expected 200, got {status} with {body}"
    );
}

/// Every identity the account holds of the class `identity_type`, following the
/// listing's cursor to the end.
async fn listed_identities(
    router: &MultiServiceRouter,
    case: &str,
    identity_type: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut params: Vec<Field> = vec![
            field("IdentityType", identity_type),
            field("MaxItems", PAGE_SIZE.to_string()),
        ];
        if let Some(token) = &token {
            params.push(field("NextToken", token));
        }
        let (status, body) = call(router, "ListIdentities", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListIdentities(IdentityType={identity_type:?}) expected 200, got \
             {status} with {body}"
        );
        let page = element(&body, "Identities")
            .map(|list| elements(&list, "member"))
            .unwrap_or_default();
        assert!(
            page.len() <= PAGE_SIZE,
            "case {case:?}: a page of {PAGE_SIZE} may carry at most that many identities, got \
             {page:?}"
        );
        seen.extend(page);
        pages += 1;
        token = element(&body, "NextToken");
        if token.is_none() {
            break;
        }
        assert!(
            pages <= MAX_IDENTITIES_PER_CALL,
            "case {case:?}: ListIdentities(IdentityType={identity_type:?}) did not terminate, \
             saw {seen:?}"
        );
    }
    seen
}

/// What the account reports about one identity: the class it was listed under, its
/// verification state and its TXT verification token.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    identity: String,
    identity_type: String,
    status: String,
    token: Option<String>,
}

/// The pre-flight an SES client performs to find a sender: page `ListIdentities` for
/// each class the model declares, then read the verification of everything it listed,
/// in batches no larger than the bound this report documents.
async fn discover(router: &MultiServiceRouter, case: &str) -> Vec<Reported> {
    let mut reported: Vec<Reported> = Vec::new();
    // The classes are the model's own `IdentityType` value set, not a list written
    // here: `IDENTITY_TYPE_VALUES` is generated from the model.
    for identity_type in IDENTITY_TYPE_VALUES {
        let listed = listed_identities(router, case, identity_type).await;
        for batch in listed.chunks(MAX_IDENTITIES_PER_CALL) {
            let params: Vec<Field> = batch
                .iter()
                .enumerate()
                .map(|(index, identity)| {
                    field(format!("Identities.member.{}", index + 1), identity.clone())
                })
                .collect();
            let (status, body) =
                call(router, "GetIdentityVerificationAttributes", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetIdentityVerificationAttributes({batch:?}) expected 200, got \
                 {status} with {body}"
            );
            let attributes = element(&body, "VerificationAttributes").unwrap_or_default();
            for (identity, value) in map_entries(&attributes) {
                reported.push(Reported {
                    identity,
                    identity_type: (*identity_type).to_string(),
                    status: element(&value, "VerificationStatus").unwrap_or_default(),
                    token: element(&value, "VerificationToken"),
                });
            }
        }
    }
    reported
}

/// The six members `CreateCustomVerificationEmailTemplate` carries, in the order the
/// model lists them.
fn create_params(name: &str, sender: &str) -> Vec<Field> {
    vec![
        field("TemplateName", name),
        field("FromEmailAddress", sender),
        field("TemplateSubject", "Please verify your address"),
        field(
            "TemplateContent",
            "<html><body>Please verify your address.</body></html>",
        ),
        field("SuccessRedirectionURL", "https://example.com/verified"),
        field("FailureRedirectionURL", "https://example.com/failed"),
    ]
}

/// `CreateCustomVerificationEmailTemplate`.
async fn create_template(
    router: &MultiServiceRouter,
    name: &str,
    sender: &str,
) -> (u16, String) {
    call(
        router,
        "CreateCustomVerificationEmailTemplate",
        &create_params(name, sender),
    )
    .await
}

/// The `FromEmailAddress` `GetCustomVerificationEmailTemplate` reports for `name`.
async fn stored_sender(router: &MultiServiceRouter, case: &str, name: &str) -> Option<String> {
    let (status, body) = call(
        router,
        "GetCustomVerificationEmailTemplate",
        &[field("TemplateName", name)],
    )
    .await;
    if status != 200 {
        return None;
    }
    let stored = element(&body, "FromEmailAddress").map(|raw| xml_text(&raw));
    assert!(
        stored.is_some(),
        "case {case:?}: the read of {name:?} must report its FromEmailAddress, got {body}"
    );
    stored
}

/// The entry `discover` reported for `identity`, by the spelling it was recorded with.
fn entry_for<'a>(reported: &'a [Reported], identity: &str) -> Option<&'a Reported> {
    reported
        .iter()
        .find(|entry| entry.identity == identity)
}

#[tokio::test]
async fn a_verified_identity_is_discovered_through_the_type_partitioned_verification_reads() {
    let (router, engine) = emulator_and_engine();
    for row in rows() {
        request_verification(&router, row.case, row.identity).await;
    }

    // Asking for verification only STARTS it. Every identity is already listed under
    // its own class, and reported in the state it is actually in, so the pre-flight
    // sees the account's identities but resolves NO sender from them.
    let outstanding = discover(&router, "before the verifications come through").await;
    for row in rows() {
        let case = row.case;
        let Some(entry) = entry_for(&outstanding, row.identity) else {
            panic!(
                "case {case:?}: expected the account to report {:?}, got {outstanding:?}",
                row.identity
            );
        };
        assert_eq!(
            (entry.identity_type.as_str(), entry.status.as_str()),
            (row.identity_type, PENDING),
            "case {case:?}: expected {:?} under IdentityType {:?} reading {PENDING}, got {entry:?}",
            row.identity,
            row.identity_type
        );
    }
    let verified: Vec<&Reported> = outstanding
        .iter()
        .filter(|entry| entry.status == SUCCESS)
        .collect();
    assert!(
        verified.is_empty(),
        "case before the verifications come through: no identity may read {SUCCESS} yet, got \
         {verified:?}"
    );

    // Once the move the clock owes each record falls due, the SAME reads report every
    // one of them verified: that is what makes it a sending identity.
    settle_verifications(&engine);
    let discovered = discover(&router, "once the verifications come through").await;
    assert_eq!(
        discovered.len(),
        rows().len(),
        "case once the verifications come through: expected one entry per identity the account \
         holds, got {discovered:?}"
    );
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let Some(entry) = entry_for(&discovered, row.identity) else {
            panic!(
                "case {case:?}: expected the account to report {:?}, got {discovered:?}",
                row.identity
            );
        };
        assert_eq!(
            (entry.identity_type.as_str(), entry.status.as_str()),
            (row.identity_type, SUCCESS),
            "case {case:?}: expected {:?} under IdentityType {:?} reading {SUCCESS}, got {entry:?}",
            row.identity,
            row.identity_type
        );
        // "The verification token for a domain identity. Null for email address
        // identities" (`IdentityVerificationAttributes$VerificationToken`).
        match row.identity_type {
            "Domain" => {
                let token = entry.token.clone().unwrap_or_default();
                let decoded = ::aws_smithy_types::base64::decode(&token)
                    .map(|bytes| bytes.len())
                    .unwrap_or_default();
                assert_eq!(
                    decoded, VERIFICATION_TOKEN_BYTES,
                    "case {case:?}: expected a TXT token decoding to {VERIFICATION_TOKEN_BYTES} \
                     bytes, got {token:?}"
                );
            }
            _ => assert!(
                entry.token.is_none(),
                "case {case:?}: an address identity carries no VerificationToken, got {:?}",
                entry.token
            ),
        }

        // And a sender the discovered identity covers is admitted, with the read
        // echoing it: the sender gate is open for exactly the identities this walk
        // resolves.
        let name = format!("discovered-sender-{index}");
        let (status, body) = create_template(&router, &name, row.sender).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for the sender {:?} of a verified identity, got \
             {status} with {body}",
            row.sender
        );
        let stored = stored_sender(&router, case, &name).await;
        assert_eq!(
            stored.as_deref(),
            Some(row.sender),
            "case {case:?}: expected the read to echo the sender {:?}, got {stored:?}",
            row.sender
        );
    }
}

#[tokio::test]
async fn an_identity_whose_verification_is_still_outstanding_is_listed_but_is_no_sender() {
    let (router, engine) = emulator_and_engine();
    // Two identities the account has verified, one of each class, so the walk below
    // runs against an account that does hold senders.
    for identity in ["sender@example.com", "example.net"] {
        request_verification(&router, "the verified identities", identity).await;
    }
    settle_verifications(&engine);

    // And one of each class asked for AFTER that, so its own move is still owed. The
    // rows differ in class and in the form of the name, which is what the rule
    // generalises over; what they share is that the account's verification of them has
    // not come through.
    let outstanding: &[(&str, &str, &str)] = &[
        ("a mailbox asked for later", "late@example.test", "late@example.test"),
        ("a domain asked for later", "late.example.test", "noreply@late.example.test"),
    ];
    for (case, identity, _) in outstanding {
        request_verification(&router, case, identity).await;
    }

    let discovered = discover(&router, "a part-verified account").await;
    // The identities the account did verify are still the senders it has.
    for identity in ["sender@example.com", "example.net"] {
        let entry = entry_for(&discovered, identity);
        assert_eq!(
            entry.map(|entry| entry.status.as_str()),
            Some(SUCCESS),
            "case the verified identities: expected {identity:?} to keep reading {SUCCESS}, got \
             {entry:?}"
        );
    }
    for (case, identity, sender) in outstanding {
        // Listed — the account holds the identity — and reported in the state it is
        // in, which is not a sender's.
        let Some(entry) = entry_for(&discovered, identity) else {
            panic!("case {case:?}: expected the account to list {identity:?}, got {discovered:?}");
        };
        assert_eq!(
            entry.status, PENDING,
            "case {case:?}: expected {identity:?} to read {PENDING}, got {entry:?}"
        );
        let (status, body) = create_template(&router, "outstanding", sender).await;
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (status, code.as_deref(), message.as_deref()),
            (
                400,
                Some(NOT_VERIFIED),
                Some(format!("The from email address {sender} is not verified.").as_str())
            ),
            "case {case:?}: expected the unverified-sender refusal naming {sender:?}, got \
             {status} with {body}"
        );
    }
}
