// SPDX-License-Identifier: CC0-1.0

//! WHICH requests this service claims — the step before any handler, and the one that
//! decides whether this service is reachable at all.
//!
//! The rule these cases pin: a service whose SigV4 signing name is SHARED with other
//! emulated services claims a request only when the request addresses THIS service —
//! the resolved routing key names one of the names its own endpoint answers on (its
//! signing name, or its endpoint prefix for a host-addressed request) AND the request
//! arrives in this service's own wire form, which for a REST service is the URI path
//! space its model binds. A claim made on the signing name alone makes every service
//! sharing that name claim every request, which the router refuses pre-dispatch rather
//! than serving by an arbitrary one — so the whole surface becomes unreachable, and the
//! refusal carries a code no AWS model declares.
//!
//! Four emulated services sign as `ses`
//! (`smithy_query(query="service_info")` on each model): this one
//! (`SimpleEmailService_v2`, `restJson1`, endpoint `email`, `/v2/email/…`), SES v1
//! (`SimpleEmailService`, `awsQuery`, endpoint `email`, every operation a `POST /` with
//! the operation in the `Action` form field), Pinpoint Email
//! (`AmazonPinpointEmailService`, `restJson1`, endpoint `email`, `/v1/email/…`) and
//! MailManager (`MailManagerSvc`, `awsJson1_0` + `rpcv2Cbor`, endpoint `mail-manager`).
//! The rows below span what the rule generalises over: both routing signals (a SigV4
//! credential scope and a host-style endpoint), both of the names this endpoint answers
//! on, several of this service's own routes (fixed segments, a path label, a query
//! string), each peer's wire form, a `/v3/…` path no model in the family binds, a routing
//! key naming another service entirely, and this service's own path space addressed at an
//! endpoint it does not answer on.
//!
//! Claiming is decided from the request HEAD alone, so each row is checked against
//! [`smithy_server_runtime::Service::can_handle`] directly; what the claimed requests are
//! then ANSWERED with is the business of the suites beside this one.

use std::sync::Arc;

use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::Service;

/// The region every row signs for and addresses.
const REGION: &str = "us-east-1";

/// One request head, and whether this service must claim it.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    method: &'static str,
    /// Path (and query) only; the host is supplied separately.
    uri: String,
    /// The `Host` header — the host-style routing signal, read for a request that
    /// carries no SigV4 credential scope.
    host: String,
    /// The signing name a SigV4 credential scope names, or `None` for a request that
    /// must route by host alone.
    scope: Option<&'static str>,
    /// The `X-Amz-Target` an `awsJson` client names its operation in.
    target: Option<&'static str>,
    content_type: Option<&'static str>,
    /// Whether this service must claim the request.
    claimed: bool,
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature itself is never read by a claim decision.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The SES endpoint host, which three of the four services answer on.
fn email_host() -> String {
    format!("email.{REGION}.amazonaws.com")
}

/// MailManager's own endpoint host.
fn mail_manager_host() -> String {
    format!("mail-manager.{REGION}.amazonaws.com")
}

/// A request signed as `ses` and addressed at the SES endpoint: the common form, which
/// every peer sees the same routing key for.
fn signed(case: &'static str, method: &'static str, uri: &str, claimed: bool) -> Row {
    Row {
        case,
        method,
        uri: uri.to_string(),
        host: email_host(),
        scope: Some("ses"),
        target: None,
        content_type: Some("application/json"),
        claimed,
    }
}

/// The rows: a request head each.
fn rows() -> Vec<Row> {
    vec![
        // ---- This service's own routes, under its signing name ----------------------
        //
        // The URIs are the ones the model binds
        // (`smithy_query(query="operation", operation="CreateEmailIdentity,GetAccount,\
        // GetTenant,GetDedicatedIp,ListSuppressedDestinations,CreateTenant")`).
        signed(
            "CreateEmailIdentity, POST on a fixed path",
            "POST",
            "/v2/email/identities",
            true,
        ),
        signed("GetAccount, GET on a fixed path", "GET", "/v2/email/account", true),
        signed(
            "GetTenant, POST on a nested fixed path",
            "POST",
            "/v2/email/tenants/get",
            true,
        ),
        signed(
            "GetDedicatedIp, GET on a path carrying a label",
            "GET",
            "/v2/email/dedicated-ips/203.0.113.42",
            true,
        ),
        signed(
            "ListSuppressedDestinations, GET with a query string",
            "GET",
            "/v2/email/suppression/addresses?PageSize=1&NextToken=not-a-token",
            true,
        ),
        signed(
            "CreateTenant, POST with no content type at all",
            "POST",
            "/v2/email/tenants",
            true,
        ),
        // The same path space addressed at this endpoint's own prefix, with no
        // credential scope: the host-style signal resolves `email`, which this service
        // answers on, and the path is what picks it out of the three services there.
        Row {
            case: "GetAccount, host-addressed with no credential scope",
            method: "GET",
            uri: "/v2/email/account".to_string(),
            host: email_host(),
            scope: None,
            target: None,
            content_type: None,
            claimed: true,
        },
        // ---- The peers' wire forms, under the SAME routing key ----------------------
        Row {
            case: "Pinpoint Email's `/v1/…` path space",
            method: "GET",
            uri: "/v1/email/account".to_string(),
            host: email_host(),
            scope: Some("ses"),
            target: None,
            content_type: None,
            claimed: false,
        },
        Row {
            case: "SES v1's awsQuery root path",
            method: "POST",
            uri: "/".to_string(),
            host: email_host(),
            scope: Some("ses"),
            target: None,
            content_type: Some("application/x-www-form-urlencoded"),
            claimed: false,
        },
        Row {
            case: "MailManager's awsJson1_0 form on the root path",
            method: "POST",
            uri: "/".to_string(),
            host: mail_manager_host(),
            scope: Some("ses"),
            target: Some("MailManagerSvc.ListArchives"),
            content_type: Some("application/x-amz-json-1.0"),
            claimed: false,
        },
        Row {
            case: "MailManager's rpcv2Cbor URI",
            method: "POST",
            uri: "/service/MailManagerSvc/operation/ListArchives".to_string(),
            host: mail_manager_host(),
            scope: Some("ses"),
            target: None,
            content_type: Some("application/cbor"),
            claimed: false,
        },
        // ---- Neither this service's path space nor its endpoint ---------------------
        signed(
            "a `/v3/…` path no model in the family binds",
            "GET",
            "/v3/email/identities",
            false,
        ),
        Row {
            case: "this service's own path under another service's routing key",
            method: "GET",
            uri: "/v2/email/account".to_string(),
            host: email_host(),
            scope: Some("sns"),
            target: None,
            content_type: None,
            claimed: false,
        },
        Row {
            case: "this service's own path at an endpoint it does not answer on",
            method: "GET",
            uri: "/v2/email/account".to_string(),
            host: mail_manager_host(),
            scope: None,
            target: None,
            content_type: None,
            claimed: false,
        },
    ]
}

/// The request head of one row.
fn head(row: &Row) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method(row.method)
        .uri(format!("https://{}{}", row.host, row.uri))
        .header("host", &row.host);
    if let Some(scope) = row.scope {
        builder = builder.header("authorization", authorization(scope));
    }
    if let Some(target) = row.target {
        builder = builder.header("x-amz-target", target);
    }
    if let Some(content_type) = row.content_type {
        builder = builder.header("content-type", content_type);
    }
    builder
        .body(())
        .expect("a well-formed request head")
        .into_parts()
        .0
}

#[test]
fn this_service_claims_its_own_wire_form_and_nothing_else() {
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
    for row in rows() {
        let claimed = service.can_handle(&head(&row));
        assert_eq!(
            claimed, row.claimed,
            "case {:?}: expected this service to {} the request ({} {}, host {:?}, \
             credential scope {:?}, target {:?}), got claimed={claimed}",
            row.case,
            if row.claimed { "claim" } else { "decline" },
            row.method,
            row.uri,
            row.host,
            row.scope,
            row.target,
        );
    }
}
