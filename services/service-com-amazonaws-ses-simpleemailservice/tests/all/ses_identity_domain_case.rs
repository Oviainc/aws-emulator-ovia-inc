// SPDX-License-Identifier: CC0-1.0

//! Which identity of the account a request NAMES: the domain part is a DNS name.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a real
//! awsQuery request against the service. The RULE asserted, over a SET of inputs rather
//! than one recorded request: an identity of this account is named by its addr-spec with
//! the DOMAIN part compared the way DNS compares a name — without regard to case — so a
//! request that spells the domain in any case names the SAME identity, while the local
//! part of an address is compared as written and two local parts differing in case are
//! two identities. The account therefore holds ONE record per identity, under the
//! spelling it was recorded with, and that spelling is what every report answers.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the class of identity — an apex domain, a subdomain, and an email address;
//! * the spelling that recorded it against the spelling a later call names it by —
//!   lower-cased against upper-cased, mixed against lower, upper against mixed;
//! * and which operation does the naming — the verification report, the Easy DKIM
//!   report, the sending-authorization policy writer and lister, the send gate on both
//!   `SendEmail` and `SendRawEmail`, and the delete.
//!
//! The last row is the boundary the rule stops at: an address whose LOCAL part is cased
//! differently is a different identity, so the account does not hold it and a send from
//! it is refused.

use crate::harness::{
    call, element, elements, emulator_and_engine, field, map_entries, settle_verifications,
    xml_text, Field,
};
use service_com_amazonaws_ses_simpleemailservice::state::DEFAULT_REGION;
use smithy_server_runtime::MultiServiceRouter;

/// One sending-authorization policy, written the way a client sends one: a statement
/// naming the `Principal` it authorizes to send through the identity. Its `Resource`
/// names an identity by ARN, which is what the operation requires of that element.
const POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"Authorize\",\
     \"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"},\
     \"Action\":\"ses:SendEmail\",\"Resource\":\
     \"arn:aws:ses:us-east-1:000000000000:identity/example.com\"}]}";

/// One row: the identity the account records, the spellings that must name it, and the
/// sender a send then names under it.
struct Row {
    /// What makes this row different from the others.
    case: &'static str,
    /// The verification request that records the identity, as `(action, member)`.
    recorded_by: (&'static str, &'static str),
    /// The identity as the recording request spells it, which is the spelling every
    /// report answers with.
    recorded: &'static str,
    /// Other spellings of the SAME identity, which every operation must resolve to the
    /// record above.
    also_named: &'static [&'static str],
    /// The `Source` a send names under the identity, in a spelling of its own.
    sender: &'static str,
    /// Whether that sender is covered by the identity.
    covered: bool,
}

/// The rows: the identity classes, the spellings of each, and the boundary row.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an apex domain recorded in lower case",
            recorded_by: ("VerifyDomainIdentity", "Domain"),
            recorded: "case-apex.example.net",
            also_named: &["CASE-APEX.EXAMPLE.NET", "Case-Apex.Example.Net"],
            sender: "sender@CASE-APEX.example.NET",
            covered: true,
        },
        Row {
            case: "an apex domain recorded in mixed case",
            recorded_by: ("VerifyDomainIdentity", "Domain"),
            recorded: "Case-MiXeD.Example.NET",
            also_named: &["case-mixed.example.net", "CASE-MIXED.EXAMPLE.NET"],
            sender: "sender@case-mixed.example.net",
            covered: true,
        },
        Row {
            case: "a subdomain recorded in upper case",
            recorded_by: ("VerifyDomainIdentity", "Domain"),
            recorded: "MAIL.CASE-SUB.EXAMPLE.NET",
            also_named: &["mail.case-sub.example.net", "Mail.Case-Sub.Example.Net"],
            // The identity is the subdomain, so a mailbox below IT is covered too.
            sender: "sender@relay.Mail.Case-Sub.Example.Net",
            covered: true,
        },
        Row {
            case: "an email address whose domain is recorded in mixed case",
            recorded_by: ("VerifyEmailIdentity", "EmailAddress"),
            recorded: "sender@Case-Address.Example.NET",
            also_named: &[
                "sender@case-address.example.net",
                "sender@CASE-ADDRESS.EXAMPLE.NET",
            ],
            sender: "sender@case-address.example.net",
            covered: true,
        },
        Row {
            case: "an email address whose local part is cased differently",
            recorded_by: ("VerifyEmailIdentity", "EmailAddress"),
            recorded: "Sender@case-local.example.net",
            // A local part is compared as written, so this is the SAME identity only
            // when the local part matches exactly.
            also_named: &["Sender@CASE-LOCAL.EXAMPLE.NET"],
            sender: "sender@case-local.example.net",
            covered: false,
        },
    ]
}

/// The message a send from an identity this account has not verified is refused with:
/// fixed text, the Region the request was signed for upper-cased, and the identities from
/// the request.
fn unverified_message(identity: &str) -> String {
    format!(
        "Email address is not verified. The following identities failed the check in region \
         {}: {identity}",
        DEFAULT_REGION.to_uppercase()
    )
}

/// A raw message from `sender`, with the blank line SES requires between the headers and
/// the body.
fn raw_message(sender: &str, recipient: &str) -> String {
    format!("From: {sender}\r\nTo: {recipient}\r\nSubject: Case probe\r\n\r\nBody.\r\n")
}

/// The same message sent from `sender`, in each form a send of this service takes.
fn sends_from(sender: &str) -> Vec<(&'static str, &'static str, Vec<Field>)> {
    const RECIPIENT: &str = "recipient@example.org";
    let raw = ::aws_smithy_types::base64::encode(raw_message(sender, RECIPIENT));
    vec![
        (
            "SendEmail naming a Source",
            "SendEmail",
            vec![
                field("Source", sender),
                field("Destination.ToAddresses.member.1", RECIPIENT),
                field("Message.Subject.Data", "Case probe"),
                field("Message.Body.Text.Data", "Body."),
            ],
        ),
        (
            "SendRawEmail naming a Source",
            "SendRawEmail",
            vec![
                field("RawMessage.Data", raw.clone()),
                field("Source", sender),
                field("Destinations.member.1", RECIPIENT),
            ],
        ),
        (
            "SendRawEmail reading the message's own From line",
            "SendRawEmail",
            vec![field("RawMessage.Data", raw)],
        ),
    ]
}

/// One call that must succeed, as its body.
async fn accepted(router: &MultiServiceRouter, case: &str, action: &str, params: &[Field]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} expected 200, got {status} with {body}"
    );
    body
}

/// `GetIdentityVerificationAttributes` for ONE value, as
/// `(the identity the report is keyed by, its VerificationStatus)`; `None` when the
/// account holds no identity that value names.
async fn reported_verification(
    router: &MultiServiceRouter,
    case: &str,
    named: &str,
) -> Option<(String, String)> {
    let body = accepted(
        router,
        case,
        "GetIdentityVerificationAttributes",
        &[field("Identities.member.1", named)],
    )
    .await;
    map_entries(&element(&body, "VerificationAttributes").unwrap_or_default())
        .into_iter()
        .next()
        .map(|(key, value)| (key, element(&value, "VerificationStatus").unwrap_or_default()))
}

/// The identities `ListIdentities` reports, in the order it reports them.
async fn listed_identities(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let body = accepted(router, case, "ListIdentities", &[]).await;
    element(&body, "Identities")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

/// The policy names `ListIdentityPolicies` reports for `named`.
async fn listed_policies(router: &MultiServiceRouter, case: &str, named: &str) -> Vec<String> {
    let body = accepted(
        router,
        case,
        "ListIdentityPolicies",
        &[field("Identity", named)],
    )
    .await;
    element(&body, "PolicyNames")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default()
}

#[tokio::test]
async fn an_identity_is_named_by_its_domain_the_way_dns_names_one() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = emulator_and_engine();
        let (action, member) = row.recorded_by;
        accepted(&router, case, action, &[field(member, row.recorded)]).await;
        settle_verifications(&engine);

        // The account holds ONE record for the identity, under the spelling that
        // recorded it — not one per spelling a later call uses.
        for named in row.also_named {
            let reported = reported_verification(&router, case, named).await;
            assert_eq!(
                reported,
                Some((row.recorded.to_string(), "Success".to_string())),
                "case {case:?}: expected {named:?} to name the recorded identity, got {reported:?}"
            );

            // A write through one spelling and a read through another reach one record.
            accepted(
                &router,
                case,
                "PutIdentityPolicy",
                &[
                    field("Identity", *named),
                    field("PolicyName", "authorize"),
                    field("Policy", POLICY),
                ],
            )
            .await;
            let names = listed_policies(&router, case, row.recorded).await;
            assert_eq!(
                names,
                vec!["authorize".to_string()],
                "case {case:?}: expected the policy put through {named:?} on the identity \
                 recorded as {:?}, got {names:?}",
                row.recorded
            );

            // And the Easy DKIM report is keyed by the same one record.
            let body = accepted(
                &router,
                case,
                "GetIdentityDkimAttributes",
                &[field("Identities.member.1", *named)],
            )
            .await;
            let keys: Vec<String> = map_entries(&element(&body, "DkimAttributes").unwrap_or_default())
                .into_iter()
                .map(|(key, _)| key)
                .collect();
            assert_eq!(
                keys,
                vec![row.recorded.to_string()],
                "case {case:?}: expected the DKIM report of {named:?} to be keyed by {:?}, got \
                 {keys:?}",
                row.recorded
            );
        }

        let listed = listed_identities(&router, case).await;
        assert_eq!(
            listed,
            vec![row.recorded.to_string()],
            "case {case:?}: expected exactly the recorded identity, got {listed:?}"
        );

        // The send gate reads that one record too, whichever case the sender's domain
        // carries — and the boundary row, whose LOCAL part differs, is not covered.
        let expected_status = if row.covered { 200 } else { 400 };
        for (leg, action, params) in sends_from(row.sender) {
            let (status, body) = call(&router, action, &params).await;
            assert_eq!(
                status, expected_status,
                "case {case:?} through {leg}: expected {expected_status}, got {status} with {body}"
            );
            if row.covered {
                assert!(
                    element(&body, "MessageId").is_some(),
                    "case {case:?} through {leg}: an accepted send reports the id it stored, got \
                     {body}"
                );
                continue;
            }
            let code = element(&body, "Code").unwrap_or_default();
            let stated = element(&body, "Message")
                .map(|raw| xml_text(&raw))
                .unwrap_or_default();
            let expected = unverified_message(
                ::service_com_amazonaws_ses_simpleemailservice::state::addr_spec(row.sender),
            );
            assert_eq!(
                (code.as_str(), stated.as_str()),
                ("MessageRejected", expected.as_str()),
                "case {case:?} through {leg}: expected MessageRejected / {expected:?}, got \
                 {code:?} / {stated:?}"
            );
        }

        // A delete named by another spelling removes that one record.
        let deleted_by = row.also_named.first().copied().unwrap_or(row.recorded);
        accepted(
            &router,
            case,
            "DeleteIdentity",
            &[field("Identity", deleted_by)],
        )
        .await;
        let listed = listed_identities(&router, case).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected the delete named by {deleted_by:?} to remove the identity, \
             got {listed:?}"
        );
        let reported = reported_verification(&router, case, row.recorded).await;
        assert!(
            reported.is_none(),
            "case {case:?}: expected no report for the deleted identity, got {reported:?}"
        );
    }
}
