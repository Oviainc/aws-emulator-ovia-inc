// SPDX-License-Identifier: CC0-1.0

//! What a REFUSAL of this service answers, driven end to end over the awsQuery wire:
//! the whole `<ErrorResponse>` envelope, and the sentence each refusal class is worded
//! with.
//!
//! Two rules, over a TABLE of refusals rather than the recorded requests:
//!
//! 1. Every refusal NAMES THE REQUEST IT REFUSES. An `awsQuery` error is the whole
//!    envelope — `<ErrorResponse><Error>…</Error><RequestId>…</RequestId></ErrorResponse>`
//!    — so the id belongs to the envelope as a SIBLING of `<Error>`, one per response,
//!    and AWS repeats it in the response header a client may correlate on instead. No
//!    model declares that element: a success has carried its own id in
//!    `<ResponseMetadata>` all along, and this is the error-path counterpart.
//! 2. A refusal's MESSAGE is the sentence AWS words that error class with: its fixed
//!    text exactly, and its variable part — where the class has one — taken from the
//!    request.
//!
//! The rows span the planes a refusal is raised on, because each is reached through a
//! different handler: the receipt-rule-set and receipt-filter planes, all four sends
//! (the gate each of them reads is the same, and each words its own answer), the
//! tracking-options writers, the custom-verification-template reader and the
//! custom-verification send. Within one class the rows differ along what the rule
//! generalises over — which identity a send names, which domain a write asks to
//! redirect through, which template name a read asks for — so a message with a
//! variable part is pinned over a SET of values and a message without one is pinned to
//! be unchanged by them.
//!
//! The LAYERS that can render an envelope (a framework refusal, a routing failure, the
//! scaffold signal) are the subject of `ses_query_error_envelope.rs`; the wording of
//! each class on its own is pinned in the suite that owns that class. What this suite
//! adds is the two rules together, on the calls a client actually refuses on.

use crate::harness::{
    call_keeping_headers, element, emulator_and_engine, error_envelope, field, is_request_id,
    occurrences, settle_verifications, Field, REQUEST_ID_HEADER,
};
use smithy_server_runtime::MultiServiceRouter;

/// The rule-set name the account holds before the table runs, so a clone can collide
/// with it.
const LIVE_RULE_SET: &str = "live-rule-set";

/// The receipt-filter name the account holds before the table runs.
const LIVE_FILTER: &str = "live-filter";

/// The configuration set the tracking-options rows name.
const TRACKED_SET: &str = "tracked-set";

/// The one domain this account has verified, so an unverified tracking domain is a
/// domain it does NOT cover.
const VERIFIED_DOMAIN: &str = "verified.example.com";

/// The sentence AWS refuses a tracking-options write with when the custom redirect
/// domain is a domain name no verified identity of the account covers.
///
/// Fixed text `Domain <` + `> is not verified under this account.` — the angle
/// brackets are part of it — with the ONE variable part the
/// `TrackingOptions.CustomRedirectDomain` the request supplied.
fn unverified_domain(domain: &str) -> String {
    format!("Domain <{domain}> is not verified under this account.")
}

/// The sentence AWS answers for an absent custom verification email template: ENTIRELY
/// fixed text, with no variable part, and in particular no echo of the `TemplateName`
/// the request asked for.
const MISSING_TEMPLATE: &str = "Provided custom verification email template name does not exist.";

/// The sentence AWS answers while the account has not been granted production access:
/// all fixed text, its second sentence ending without a full stop.
const NO_PRODUCTION_ACCESS: &str = "Production access not granted. Please request production \
                                    access prior to sending custom verification emails";

/// One refused call: the request, the HTTP status and `<Code>` it must answer, and the
/// message where this suite pins it.
struct Row {
    /// What makes this row different from the others.
    case: String,
    /// The operation the call goes to.
    action: &'static str,
    /// The form fields that carry it.
    params: Vec<Field>,
    /// The `<Code>` a client branches on.
    code: &'static str,
    /// The sentence the refusal must state, where its class fixes it; `None` where the
    /// suite pins only the envelope and the code (the sentence is the subject of the
    /// suite that owns that class).
    message: Option<String>,
}

/// A raw message from `source`, with the blank line SES requires between the headers
/// and the body.
fn raw_message(source: &str) -> String {
    format!(
        "From: {source}\r\nTo: recipient@example.net\r\nSubject: Quarterly report\r\n\r\nSee \
         attached.\r\n"
    )
}

/// The four sends of this service, each from an identity this account has not verified
/// — the one gate every send reads, which each of them words its own answer for.
///
/// The `Source` differs per row, so the rule is pinned over a set of identities rather
/// than over one: a plain mailbox, a mailbox under a subdomain, the display-name form,
/// and a mailbox under a domain nobody asked about.
fn unverified_sends() -> Vec<Row> {
    const MESSAGE_REJECTED: &str = "MessageRejected";
    let sends: [(&str, &str, Vec<Field>); 4] = [
        (
            "SendEmail",
            "nobody@unverified.example.net",
            vec![
                field("Destination.ToAddresses.member.1", "recipient@example.net"),
                field("Message.Subject.Data", "Quarterly report"),
                field("Message.Body.Text.Data", "See attached."),
            ],
        ),
        (
            "SendRawEmail",
            "nobody@sub.unverified.example.net",
            vec![
                field(
                    "RawMessage.Data",
                    ::aws_smithy_types::base64::encode(raw_message(
                        "nobody@sub.unverified.example.net",
                    )),
                ),
                field("Destinations.member.1", "recipient@example.net"),
            ],
        ),
        (
            "SendTemplatedEmail",
            "Someone <nobody@other.example.org>",
            vec![
                field("Destination.ToAddresses.member.1", "recipient@example.net"),
                field("Template", "a-template-nobody-created"),
                field("TemplateData", "{}"),
            ],
        ),
        (
            "SendBulkTemplatedEmail",
            "nobody@never-asked-about.example",
            vec![
                field("Template", "a-template-nobody-created"),
                field("DefaultTemplateData", "{}"),
                field(
                    "Destinations.member.1.Destination.ToAddresses.member.1",
                    "recipient@example.net",
                ),
            ],
        ),
    ];
    sends
        .into_iter()
        .map(|(action, source, rest)| {
            let mut params = vec![field("Source", source)];
            params.extend(rest);
            Row {
                case: format!("{action} from the unverified identity {source}"),
                action,
                params,
                code: MESSAGE_REJECTED,
                // The sentence a send is refused with names the Region and the
                // identities that failed the check; it is pinned, over every form a
                // sending identity takes, by `ses_source_identity_gate.rs`.
                message: None,
            }
        })
        .collect()
}

/// Every refused call this suite drives, in the order it drives them.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: format!("a clone onto the rule-set name {LIVE_RULE_SET}, which the account holds"),
            action: "CloneReceiptRuleSet",
            params: vec![
                field("OriginalRuleSetName", LIVE_RULE_SET),
                field("RuleSetName", LIVE_RULE_SET),
            ],
            code: "AlreadyExists",
            message: None,
        },
        Row {
            case: "a clone of a rule set the account does not hold".to_string(),
            action: "CloneReceiptRuleSet",
            params: vec![
                field("OriginalRuleSetName", "no-such-rule-set"),
                field("RuleSetName", "a-fresh-name"),
            ],
            code: "RuleSetDoesNotExist",
            message: None,
        },
        Row {
            case: format!("a receipt filter under {LIVE_FILTER}, a name the account holds"),
            action: "CreateReceiptFilter",
            params: vec![
                field("Filter.Name", LIVE_FILTER),
                field("Filter.IpFilter.Cidr", "10.0.0.0/24"),
                field("Filter.IpFilter.Policy", "Block"),
            ],
            code: "AlreadyExists",
            message: None,
        },
        Row {
            case: "a receipt filter whose Cidr names no range of addresses".to_string(),
            action: "CreateReceiptFilter",
            params: vec![
                field("Filter.Name", "a-fresh-filter"),
                field("Filter.IpFilter.Cidr", "not-a-cidr-block"),
                field("Filter.IpFilter.Policy", "Block"),
            ],
            code: "InvalidParameterValue",
            message: None,
        },
    ];
    rows.extend(unverified_sends());

    // The tracking-options writers, over domains no verified identity of the account
    // covers: a bare second-level domain, a subdomain of one, a subdomain of a NEIGHBOUR
    // of the verified domain (so the verified name is a prefix of it and still does not
    // cover it), and a label long enough to exercise the whole value — the domain is read
    // before the configuration set is looked up, so each is refused on the domain alone.
    for domain in [
        "unverified.example.net",
        "clicks.unverified.example.net",
        "clicks.verified.example.com.example.net",
        &format!("{}.example.net", "long-label-".repeat(4)),
    ] {
        rows.push(Row {
            case: format!("tracking options created for the unverified domain {domain}"),
            action: "CreateConfigurationSetTrackingOptions",
            params: vec![
                field("ConfigurationSetName", TRACKED_SET),
                field("TrackingOptions.CustomRedirectDomain", domain),
            ],
            code: "InvalidTrackingOptions",
            message: Some(unverified_domain(domain)),
        });
    }
    for domain in ["repointed.example.net", "clicks.repointed.example.org"] {
        rows.push(Row {
            case: format!("tracking options updated to the unverified domain {domain}"),
            action: "UpdateConfigurationSetTrackingOptions",
            params: vec![
                field("ConfigurationSetName", TRACKED_SET),
                field("TrackingOptions.CustomRedirectDomain", domain),
            ],
            code: "InvalidTrackingOptions",
            message: Some(unverified_domain(domain)),
        });
    }

    // A custom verification email template read by a name the account never created.
    // The sentence is fixed, so the rows differ only in the name they ask for — which
    // the message must not reflect back.
    for name in [
        "never-created",
        "987654321",
        "absent-ünïcödé-名前",
        &"absent-".repeat(8),
    ] {
        rows.push(Row {
            case: format!("a custom verification email template read by the absent name {name}"),
            action: "GetCustomVerificationEmailTemplate",
            params: vec![field("TemplateName", name)],
            code: "CustomVerificationEmailTemplateDoesNotExist",
            message: Some(MISSING_TEMPLATE.to_string()),
        });
    }

    // A custom verification email sent while the account is still in the sandbox. The
    // sentence is fixed too, so these rows differ in the address and the template they
    // name.
    for (address, template) in [
        ("new@example.org", "confirm-your-address"),
        ("Someone <new@example.net>", "another-template"),
    ] {
        rows.push(Row {
            case: format!("a custom verification email sent to {address} in the sandbox"),
            action: "SendCustomVerificationEmail",
            params: vec![
                field("EmailAddress", address),
                field("TemplateName", template),
            ],
            code: "ProductionAccessNotGranted",
            message: Some(NO_PRODUCTION_ACCESS.to_string()),
        });
    }

    rows
}

#[tokio::test]
async fn every_refused_call_names_the_request_and_states_its_own_sentence() {
    let (router, engine) = emulator_and_engine();

    // What the account holds before the table runs: the one verified domain a tracking
    // domain would have to be covered by, the rule set a clone collides with, the
    // filter a second create collides with, and the configuration set the
    // tracking-options rows name.
    seed(
        &router,
        "VerifyDomainIdentity",
        &[field("Domain", VERIFIED_DOMAIN)],
    )
    .await;
    settle_verifications(&engine);
    seed(
        &router,
        "CreateReceiptRuleSet",
        &[field("RuleSetName", LIVE_RULE_SET)],
    )
    .await;
    seed(
        &router,
        "CreateReceiptFilter",
        &[
            field("Filter.Name", LIVE_FILTER),
            field("Filter.IpFilter.Cidr", "198.51.100.0/24"),
            field("Filter.IpFilter.Policy", "Block"),
        ],
    )
    .await;
    seed(
        &router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", TRACKED_SET)],
    )
    .await;

    // Every request id this service reported: one per response, so no two are equal.
    let mut request_ids: Vec<(String, String)> = Vec::new();

    for row in rows() {
        let answer = call_keeping_headers(&router, row.action, &row.params).await;
        let case = &row.case;
        let status = answer.status;
        let body = &answer.body;
        // The two elements every suite that asserts a refusal reads, with the message's
        // escaped entities read back ([`crate::harness::error_envelope`]).
        let (code, stated) = error_envelope(body);
        let stated = stated.unwrap_or_default();

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            occurrences(body, "<ErrorResponse"),
            1,
            "case {case:?}: expected exactly one <ErrorResponse> envelope, got {body}"
        );
        assert_eq!(
            code.as_deref(),
            Some(row.code),
            "case {case:?}: expected the error code {}, got {status} with {body}",
            row.code
        );

        match &row.message {
            Some(expected) => assert_eq!(
                &stated, expected,
                "case {case:?}: expected the message {expected:?}, got {stated:?}"
            ),
            None => assert!(
                !stated.is_empty(),
                "case {case:?}: expected a non-empty <Message>, got {status} with {body}"
            ),
        }

        // RULE 1: the envelope names the request it refuses, exactly once, in the form
        // an AWS request id has — as a SIBLING of `<Error>`, which is what an SDK reads
        // as the failed call's request id, and in the header beside it.
        assert_eq!(
            occurrences(body, "<RequestId>"),
            1,
            "case {case:?}: expected exactly one <RequestId> in the envelope, got {body}"
        );
        let request_id = element(body, "RequestId").unwrap_or_default();
        assert!(
            is_request_id(&request_id),
            "case {case:?}: expected a request id of the form 8-4-4-4-12 lowercase hex, got \
             {request_id:?} in {body}"
        );
        let error = element(body, "Error").unwrap_or_default();
        assert!(
            !error.contains("<RequestId>"),
            "case {case:?}: the request id belongs to the envelope, not to <Error>, got {body}"
        );
        assert_eq!(
            answer.request_id_header, request_id,
            "case {case:?}: expected the {REQUEST_ID_HEADER} header to carry the envelope's id \
             {request_id:?}, got {:?}",
            answer.request_id_header
        );
        request_ids.push((row.case.clone(), request_id));
    }

    for (case, id) in &request_ids {
        let shared: Vec<&str> = request_ids
            .iter()
            .filter(|(other_case, other)| other_case != case && other == id)
            .map(|(other_case, _)| other_case.as_str())
            .collect();
        assert!(
            shared.is_empty(),
            "case {case:?}: a request id names ONE response, but {id} is also reported by \
             {shared:?}"
        );
    }
}

/// One call the table needs to have succeeded before it runs.
async fn seed(router: &MultiServiceRouter, action: &str, params: &[Field]) {
    let (status, body) = crate::harness::call(router, action, params).await;
    assert_eq!(
        status, 200,
        "seeding {action}: expected HTTP 200, got {status} with {body}"
    );
}
