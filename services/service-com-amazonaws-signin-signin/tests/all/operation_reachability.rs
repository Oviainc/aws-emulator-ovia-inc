// SPDX-License-Identifier: CC0-1.0

//! Every operation this service's model binds is served on the one endpoint the
//! emulator is reached on — whatever region the caller signed for, whatever host it
//! addressed — and no answer announces another address.
//!
//! This is the rule the resource-policy and console-authorization surfaces were
//! reported unreachable against: a client that reached the emulator for the `/v1/…`
//! OAuth 2.0 paths was sent to a public regional host for `POST
//! /get-resource-policy` and for the console-authorization paths, and for the latter
//! only when it signed for `us-east-1` (the only region AWS accepts those writes in).
//! So the dimensions the rows differ along are exactly those: the URI path, the
//! credential scope's region, and the `Host` header. Every row must be answered by THIS
//! service — a success, its own modelled error, or its own scaffold `NotImplemented` —
//! and never by a routing failure (404 `UnknownService`, 500 `AmbiguousService`), since
//! an operation the emulator does not serve on the endpoint it is reached on is exactly
//! what leaves a client resolving some other address.

use serde_json::{json, Value};

use crate::signin_harness::{dispatch, emulator, Answer, EMULATOR_ACCOUNT};

/// A well-formed `StatementId` (`^[A-Za-z0-9+/]{64}=?$`) this service never minted, so
/// the delete reaches its own not-found answer rather than a boundary refusal.
const UNMINTED_STATEMENT_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// A well-formed `IntrospectionToken` (`^ASO[AR][A-Za-z0-9+/=_\-]+$`) this service never
/// minted.
const UNMINTED_ACCESS_TOKEN: &str = "ASOAaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The two routing failures the multi-service router answers with. Neither is an answer
/// from a service, so neither may appear in any row.
const ROUTING_FAILURES: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// One call of the sequence: the operation, its route, the request body, and every
/// answer this service may owe it.
///
/// `accepted` is a SET rather than one pair because the rule under test is "this
/// service answered", not "this service answered exactly this": a surface AWS
/// region-locks may answer its own modelled `ValidationException` to a write from
/// another region, and that is still this service answering. What is never accepted is
/// a routing failure.
struct Call<'a> {
    operation: &'a str,
    uri: &'a str,
    body: Value,
    accepted: &'a [(u16, Option<&'a str>)],
}

/// The answer of a call that succeeded.
const SERVED: &[(u16, Option<&str>)] = &[(200, None)];

/// The answers of a console-authorization call: its success, or this service's own
/// modelled refusals — a `ValidationException` (which is what a write AWS region-locks
/// answers from another region) or, for a read whose write was refused that way, its
/// `ResourceNotFoundException`.
const SERVED_OR_REFUSED_BY_THIS_SERVICE: &[(u16, Option<&str>)] = &[
    (200, None),
    (400, Some("ValidationException")),
    (404, Some("ResourceNotFoundException")),
];

/// Every operation the model binds, in an order each one's own answer is determined in:
/// each reader runs before the write that changes what it reports.
///
/// Source: the service's 11 bound operations and their `@http` routes, as
/// `OPERATIONS.md` lists them — 12 rows, because `GetConsoleAuthorizationConfiguration`
/// is called on both sides of the put. `CreateOAuth2Token` (the anonymous `/v1/token`
/// route) is still a scaffold stub filed as its own issue, so the answer it owes is
/// this service's own 501 — which is still this service answering, which is what this
/// suite is about.
fn sequence() -> Vec<Call<'static>> {
    vec![
        Call {
            operation: "GetResourcePolicy",
            uri: "/get-resource-policy",
            body: json!({}),
            accepted: SERVED,
        },
        Call {
            operation: "ListResourcePermissionStatements",
            uri: "/list-resource-permission-statements",
            body: json!({"maxResults": 25}),
            accepted: SERVED,
        },
        Call {
            operation: "PutResourcePermissionStatement",
            uri: "/put-resource-permission-statement",
            body: json!({"sourceIp": "203.0.113.1"}),
            accepted: SERVED,
        },
        Call {
            operation: "DeleteResourcePermissionStatement",
            uri: "/delete-resource-permission-statement",
            body: json!({"statementId": UNMINTED_STATEMENT_ID}),
            accepted: &[(404, Some("ResourceNotFoundException"))],
        },
        Call {
            operation: "GetConsoleAuthorizationConfiguration (before the put)",
            uri: "/get-console-authorization-configuration",
            body: json!({"targetId": EMULATOR_ACCOUNT}),
            accepted: &[(404, Some("ResourceNotFoundException"))],
        },
        Call {
            operation: "PutConsoleAuthorizationConfiguration",
            uri: "/put-console-authorization-configuration",
            body: json!({"targetId": EMULATOR_ACCOUNT}),
            accepted: SERVED_OR_REFUSED_BY_THIS_SERVICE,
        },
        Call {
            operation: "GetConsoleAuthorizationConfiguration (after the put)",
            uri: "/get-console-authorization-configuration",
            body: json!({"targetId": EMULATOR_ACCOUNT}),
            accepted: SERVED_OR_REFUSED_BY_THIS_SERVICE,
        },
        Call {
            operation: "DeleteConsoleAuthorizationConfiguration",
            uri: "/delete-console-authorization-configuration",
            body: json!({"targetId": EMULATOR_ACCOUNT}),
            accepted: SERVED_OR_REFUSED_BY_THIS_SERVICE,
        },
        Call {
            operation: "CreateOAuth2TokenWithIAM",
            uri: "/v1/token?x-amz-client-auth-method=iam",
            body: json!({"grant_type": "client_credentials", "resource": "aws-mcp.amazonaws.com"}),
            accepted: SERVED,
        },
        Call {
            operation: "IntrospectOAuth2TokenWithIAM",
            uri: "/v1/introspect?x-amz-client-auth-method=iam",
            body: json!({"token": UNMINTED_ACCESS_TOKEN}),
            accepted: SERVED,
        },
        Call {
            operation: "RevokeOAuth2TokenWithIAM",
            uri: "/v1/revoke?x-amz-client-auth-method=iam",
            body: json!({"token": ""}),
            accepted: SERVED,
        },
        Call {
            operation: "CreateOAuth2Token (a scaffold stub: this service's own 501)",
            uri: "/v1/token",
            body: json!({"clientId": "arn:aws:signin:::devtools/same-device",
                         "grantType": "refresh_token",
                         "refreshToken": "ASORaaaaaaaaaaaaaaaaaaaa"}),
            accepted: &[(501, Some("NotImplemented"))],
        },
    ]
}

/// Nothing in an answer may carry an address the caller would dial next. Checked on
/// every row: a URL in a response, or a redirect, is how a client ends up resolving a
/// different host for the next call. Only an HTTP(S) URL and a `Location` header count:
/// a bare identifier a claim names (a token's `iss`, say) is not an address, and a
/// member whose own value is a URL would be a different rule.
fn announces_no_address(case: &str, answer: &Answer) {
    assert!(
        answer.headers.get("location").is_none(),
        "case {case:?}: an answer must not redirect the caller elsewhere, got Location \
         {:?}",
        answer.headers.get("location")
    );
    let body = String::from_utf8_lossy(&answer.raw_body);
    for scheme in ["http://", "https://"] {
        assert!(
            !body.contains(scheme),
            "case {case:?}: an answer must announce no {scheme} URL, got {body}"
        );
    }
}

#[tokio::test]
async fn every_operation_is_served_on_the_endpoint_it_is_reached_on() {
    // The credential scope's region: the default the behavioural suite's client uses,
    // the only region AWS accepts a console-authorization write in, and two more from
    // other partition-free regions, so no row can be passing because of one region.
    let regions = ["us-west-2", "us-east-1", "eu-central-1", "ap-southeast-2"];
    // The `Host` header: the loopback address an emulator run reaches, a bare name, the
    // regional AWS host and the public dual-stack host the report saw a client sent to.
    // Each must be served the same, because the emulator is reached on one endpoint
    // whatever the client addressed.
    let hosts = [
        "127.0.0.1:9297",
        "localhost",
        "signin.us-east-1.amazonaws.com",
        "signin.us-west-2.api.aws",
    ];

    for region in regions {
        for host in hosts {
            // A fresh emulator per endpoint row, so each row's sequence starts from an
            // empty store and every answer is determined.
            let router = emulator();
            for call in sequence() {
                let case = format!(
                    "{} signed for {region} addressed to {host}",
                    call.operation
                );
                let answer = dispatch(&router, call.uri, call.body.clone(), region, host).await;
                let code = answer.error_code();
                assert!(
                    !ROUTING_FAILURES.contains(&code),
                    "case {case:?}: the request must reach this service, but the router \
                     answered {code} ({}) with {:?}",
                    answer.status,
                    answer.body
                );
                let reported = (answer.status, (!code.is_empty()).then_some(code));
                assert!(
                    call.accepted.contains(&reported),
                    "case {case:?}: expected one of {:?} from this service, got {} {:?}",
                    call.accepted,
                    answer.status,
                    answer.body
                );
                announces_no_address(&case, &answer);
            }
        }
    }
}
