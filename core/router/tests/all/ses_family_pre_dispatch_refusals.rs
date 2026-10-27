// SPDX-License-Identifier: CC0-1.0

//! The refusals the gateway renders BEFORE a handler runs, for the four emulated services
//! that share the SigV4 signing name `ses`.
//!
//! The harness is the sibling suite's — [`crate::ses_family_signing_name_routing`] owns
//! the [`Call`] description, the [`Expect`] expectation, the router-refusal names, the
//! endpoint hosts and the send, so the two suites cannot describe a request to this
//! family, or disagree about what a correct answer to one is. What this suite adds are
//! the two things that suite does not reach: the SESv2 surface a client configures a
//! sending account through, and what each of the four protocols is answered with when the
//! refusal comes before any operation runs.
//!
//! Two rules:
//!
//! 1. A request reaches EXACTLY ONE emulated service: the one whose endpoint and wire
//!    form it addresses. A client addresses one service behind one endpoint, so the
//!    answer must be that service's own — a modelled success or a modelled client error —
//!    and never one of the router's own pre-dispatch refusals, which name no AWS shape
//!    (see [`ROUTER_REFUSALS`]). A SigV4 signing name is not an identity: four mounted
//!    services sign as `ses`, so a claim made on the signing name alone makes all four
//!    claim every request and the whole surface unreachable — and on a 5xx, which an SDK
//!    retries to its ceiling instead of surfacing.
//! 2. A refusal rendered before the handler runs — an authentication refusal, a routing
//!    failure, a transport failure — is framed in the ADDRESSED service's own protocol.
//!    The framing of a response is fixed by the model, not by the layer that produced it:
//!    an `awsQuery` service answers every response, success or failure, as XML (the
//!    `<ErrorResponse><Error><Code>…` document its SDK reads a code out of), while a
//!    `restJson1` or `awsJson` service answers with a JSON document. A JSON body served
//!    to a query-protocol caller is not an error it can classify at all: its parser
//!    rejects the document, so the client sees no status/code pair and no code to stop
//!    retrying on.
//!
//! The first table spans the SESv2 surface a client configures a sending account through
//! — identities, configuration sets, contact lists, tenants, dedicated IP pools,
//! multi-region endpoints, the account itself, the deliverability and VDM reports, the
//! suppression list — over both HTTP methods, paths with and without labels and query
//! strings, both routing signals, and two of its modelled refusal legs. The second table
//! spans the protocols: it drives one credential this emulator never minted at each of
//! the four services and pins the framing each one's SDK can read a code out of.
//!
//! This suite needs the WHOLE router: the ambiguity only exists when every service is
//! mounted, so a single-service test cannot see it. Assembling the router costs stack
//! (see `boot_serve.rs`), so it is built — and the requests driven — on a sized thread.
//!
//! One state is deliberately NOT among the rows: a refusal raised before any service is
//! CLAIMED, which is where the recorded JSON-framed refusal came from. With rule 1
//! holding, a request addressing one of these services always has exactly one claimant,
//! so that state is unreachable for this family by construction — the first table is what
//! proves it unreachable, and the second pins the framing of the refusals that remain,
//! every one of them rendered after a service is claimed and before its handler runs. A
//! request that addresses NO service at all (the sibling suite's unclaimed rows) is still
//! answered by the router's own envelope rather than any service's, because no protocol
//! has been resolved at that point; that is a limit of this suite, not a claim it makes.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::ses_family_signing_name_routing::{
    email_host, error_code, mail_manager_host, send, Call, Expect, BOOT_STACK_SIZE,
    ROUTER_REFUSALS,
};

/// A SESv2 call: signed as `ses` and addressed at the SES endpoint, which is the form a
/// client's own SDK sends.
fn sesv2(case: &'static str, method: &'static str, uri: &str, body: &str) -> Call {
    Call {
        case,
        method,
        uri: uri.to_string(),
        host: email_host(),
        signing_name: Some("ses"),
        content_type: Some("application/json"),
        target: None,
        body: body.to_string(),
    }
}

/// An answer that reached its service: no status or code is pinned, because the operation
/// behind it may still be a scaffold, but the envelope it arrives in is.
///
/// A row that owes this is pinning rule 1 only. What each of those operations ANSWERS is
/// its own issue's business — the ones still scaffolded are owned by their own stub
/// reports — so a row here must keep passing both before and after that work lands.
fn reached() -> Expect {
    Expect {
        status: None,
        code: None,
        xml: false,
        contains: &[],
    }
}

/// The SESv2 surface this family's clients configure an account through: one request per
/// operation, each of which must reach SESv2 and be answered in its own envelope.
///
/// The methods and URIs are the ones the model binds (`smithy_query(query="operation",
/// operation="CreateEmailIdentity,CreateConfigurationSet,CreateContactList,CreateTenant,\
/// CreateDedicatedIpPool,CreateMultiRegionEndpoint,GetAccount,GetBlacklistReports,\
/// GetEmailAddressInsights,ListRecommendations,ListSuppressedDestinations,GetTenant,\
/// GetDedicatedIp")`).
fn surface_rows() -> Vec<(Call, Expect)> {
    vec![
        (
            sesv2(
                "CreateEmailIdentity",
                "POST",
                "/v2/email/identities",
                r#"{"EmailIdentity":"router-ses-family.example.com"}"#,
            ),
            reached(),
        ),
        (
            sesv2(
                "CreateConfigurationSet",
                "POST",
                "/v2/email/configuration-sets",
                r#"{"ConfigurationSetName":"router-ses-family-set"}"#,
            ),
            reached(),
        ),
        (
            sesv2(
                "CreateContactList",
                "POST",
                "/v2/email/contact-lists",
                r#"{"ContactListName":"router-ses-family-list"}"#,
            ),
            reached(),
        ),
        (
            sesv2(
                "CreateTenant",
                "POST",
                "/v2/email/tenants",
                r#"{"TenantName":"router-ses-family-tenant"}"#,
            ),
            reached(),
        ),
        (
            sesv2(
                "CreateDedicatedIpPool",
                "POST",
                "/v2/email/dedicated-ip-pools",
                r#"{"PoolName":"router-ses-family-pool"}"#,
            ),
            reached(),
        ),
        (
            sesv2(
                "CreateMultiRegionEndpoint",
                "POST",
                "/v2/email/multi-region-endpoints",
                r#"{"EndpointName":"router-ses-family-endpoint",
                    "Details":{"RoutesDetails":[{"Region":"eu-west-1"}]}}"#,
            ),
            reached(),
        ),
        (
            sesv2("GetAccount", "GET", "/v2/email/account", ""),
            // Implemented, and an answer only SESv2 can give on this path: its account
            // report always carries the per-day and per-second sending quota
            // (`smithy_query(query="operation", operation="GetAccount")`: `SendQuota`).
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &["SendQuota"],
            },
        ),
        (
            sesv2(
                "GetBlacklistReports, a query-bound list member",
                "GET",
                "/v2/email/deliverability-dashboard/blacklist-report\
                 ?BlacklistItemNames=203.0.113.42",
                "",
            ),
            reached(),
        ),
        (
            sesv2(
                "GetEmailAddressInsights",
                "POST",
                "/v2/email/email-address-insights",
                r#"{"EmailAddress":"recipient@router-ses-family.example.com"}"#,
            ),
            reached(),
        ),
        (
            sesv2(
                "ListRecommendations",
                "POST",
                "/v2/email/vdm/recommendations",
                r#"{"PageSize":1}"#,
            ),
            reached(),
        ),
        (
            sesv2(
                "ListSuppressedDestinations",
                "GET",
                "/v2/email/suppression/addresses?PageSize=1",
                "",
            ),
            reached(),
        ),
        (
            sesv2(
                "GetDedicatedIp, an IP the account does not hold",
                "GET",
                "/v2/email/dedicated-ips/203.0.113.42",
                "",
            ),
            reached(),
        ),
        // The same `/v2/…` space with no credential scope at all, addressed at the SES
        // endpoint host: the host-style signal resolves `email`, which three of the four
        // services answer on, and the path space is what picks SESv2 out of them.
        (
            Call {
                case: "GetAccount, host-addressed with no credential scope",
                method: "GET",
                uri: "/v2/email/account".to_string(),
                host: email_host(),
                signing_name: None,
                content_type: None,
                target: None,
                body: String::new(),
            },
            Expect {
                status: Some(200),
                code: None,
                xml: false,
                contains: &["SendQuota"],
            },
        ),
        // Two of this surface's modelled refusal legs: a terminal 4xx naming the
        // operation's own error shape, which is what a client gets once the request
        // reaches the service that owns it.
        (
            sesv2(
                "GetTenant for a tenant nothing created",
                "POST",
                "/v2/email/tenants/get",
                r#"{"TenantName":"router-ses-family-absent-tenant"}"#,
            ),
            Expect {
                status: Some(404),
                code: Some("NotFoundException"),
                xml: false,
                contains: &["router-ses-family-absent-tenant"],
            },
        ),
        (
            sesv2(
                "ListSuppressedDestinations with a token this listing never issued",
                "GET",
                "/v2/email/suppression/addresses?NextToken=not-a-token-this-listing-issued",
                "",
            ),
            Expect {
                status: Some(400),
                code: Some("InvalidNextTokenException"),
                xml: false,
                contains: &[],
            },
        ),
    ]
}

/// One credential this emulator never minted, driven at each of the four services: a
/// refusal rendered before any operation ran, framed by the addressed service's protocol.
///
/// Each row names an operation its service's model binds, because the refusal is reached
/// once the operation has been resolved; what differs between the rows is the protocol
/// that frames the answer. The authentication CODE is the emulator's own and is
/// deliberately not pinned — what each row requires is the terminal status, the envelope
/// the addressed service's protocol fixes, and that the code sits where that protocol's
/// client reads it (`<Code>` inside the XML `<ErrorResponse>`, `__type` in the JSON
/// document).
fn refusal_rows() -> Vec<(Call, Expect)> {
    vec![
        (
            Call {
                case: "SES v1 ListIdentities: an awsQuery caller's refusal",
                method: "POST",
                uri: "/".to_string(),
                host: email_host(),
                signing_name: Some("ses"),
                content_type: Some("application/x-www-form-urlencoded"),
                target: None,
                body: "Action=ListIdentities&Version=2010-12-01".to_string(),
            },
            Expect {
                status: Some(403),
                code: None,
                xml: true,
                contains: &["<ErrorResponse", "<Code>"],
            },
        ),
        (
            sesv2(
                "SESv2 GetAccount: a restJson1 caller's refusal",
                "GET",
                "/v2/email/account",
                "",
            ),
            Expect {
                status: Some(403),
                code: None,
                xml: false,
                contains: &["__type"],
            },
        ),
        (
            Call {
                case: "Pinpoint Email GetAccount: the other restJson1 caller's refusal",
                method: "GET",
                uri: "/v1/email/account".to_string(),
                host: email_host(),
                signing_name: Some("ses"),
                content_type: None,
                target: None,
                body: String::new(),
            },
            Expect {
                status: Some(403),
                code: None,
                xml: false,
                contains: &["__type"],
            },
        ),
        (
            Call {
                case: "MailManager ListArchives: an awsJson1_0 caller's refusal",
                method: "POST",
                uri: "/".to_string(),
                host: mail_manager_host(),
                signing_name: Some("ses"),
                content_type: Some("application/x-amz-json-1.0"),
                target: Some("MailManagerSvc.ListArchives"),
                body: "{}".to_string(),
            },
            Expect {
                status: Some(403),
                code: None,
                xml: false,
                contains: &["__type"],
            },
        ),
    ]
}

/// Check one answer against what its row owes.
fn check(call: &Call, expected: &Expect, status: u16, body: &str) {
    let case = call.case;
    let code = error_code(body);

    assert!(
        !ROUTER_REFUSALS.contains(&code.as_str()),
        "case {case:?}: expected exactly one claiming service, got the router's own \
         refusal HTTP {status} {code}: {body}"
    );
    // Nor may any answer here be a retryable 5xx: every condition these rows reach is the
    // caller's or the endpoint's, so an SDK must surface it on the first attempt. A
    // scaffolded operation's 501 is terminal and classified as such.
    assert!(
        status < 500 || status == 501,
        "case {case:?}: this family must answer with a terminal status, got HTTP \
         {status} {code}: {body}"
    );
    if expected.xml {
        assert!(
            body.trim_start().starts_with('<') && !body.contains("__type"),
            "case {case:?}: an awsQuery caller must be answered with an XML document, \
             never a JSON `__type` envelope, got HTTP {status}: {body}"
        );
    } else {
        // A JSON-protocol caller's answer must be a JSON DOCUMENT: an object carrying
        // either the operation's output or the error its envelope names. An empty body
        // and a bare JSON scalar are both answers a client can read nothing out of.
        let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
        assert!(
            parsed.as_ref().is_some_and(serde_json::Value::is_object),
            "case {case:?}: a JSON-protocol caller must be answered with a JSON \
             document, got HTTP {status}: {body}"
        );
    }
    if let Some(expected_status) = expected.status {
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {code}: {body}"
        );
    }
    if let Some(expected_code) = expected.code {
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code}, got {code:?} \
             (HTTP {status}): {body}"
        );
    }
    for needle in expected.contains {
        assert!(
            body.contains(needle),
            "case {case:?}: the answer must carry {needle:?}, got HTTP {status}: {body}"
        );
    }
}

#[test]
fn the_ses_family_answers_every_caller_in_its_own_envelope() {
    std::thread::Builder::new()
        .name("ses-family-pre-dispatch".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The surface rows exercise the SERVICES, so authentication is off: what they
            // pin is which service answered, and in what envelope.
            router.set_request_authentication(false);
            for (call, expected) in surface_rows() {
                let (status, body) = send(&router, &runtime, &call);
                check(&call, &expected, status, &body);
            }

            // From here on authentication is ON, as the emulator serves traffic, and
            // every row carries a credential the emulator never minted: each answer is a
            // refusal rendered before that service's handler ran.
            router.set_request_authentication(true);
            for (call, expected) in refusal_rows() {
                let (status, body) = send(&router, &runtime, &call);
                check(&call, &expected, status, &body);
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("every caller in this family must be answered in its own envelope");
}
