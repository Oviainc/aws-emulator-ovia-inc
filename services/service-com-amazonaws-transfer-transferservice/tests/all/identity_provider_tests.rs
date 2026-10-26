// SPDX-License-Identifier: CC0-1.0

//! What `TestIdentityProvider` answers for a stored server.
//!
//! Two rules:
//!
//! 1. The gate is the SERVER's `IdentityProviderType` and nothing else about the request:
//!    a server that delegates authentication to nothing — Transfer's own
//!    `SERVICE_MANAGED` user store — has no identity provider for this call to test and
//!    is refused with the modelled `InvalidRequestException` (400) carrying the
//!    documented "`<ServerId>` not configured for external auth"; a `ServerId` the store
//!    does not hold is a `ResourceNotFoundException`.
//! 2. A server that DOES delegate it is answered with the outcome of an authentication
//!    request against its provider: the `Url` is the endpoint that server is configured
//!    with, read off the one `IdentityProviderDetails` member its type is configured
//!    through; the `StatusCode` is "the HTTP status code that is the response from your
//!    API Gateway or your Lambda function", which is the member's modelled default `0`
//!    when no response came back; no `Response` is reported, as the operation documents
//!    for a test that got none; and the `Message` says why the test did not succeed. The
//!    emulator opens no outbound connection and runs no customer identity provider, so
//!    nothing here reports a status for a request nothing sent.
//!
//! The rows differ along what the rules generalise over: the `IdentityProviderType`
//! (each of the three external ones, and the service-managed one), whether the details
//! name the provider's endpoint at all, and the request members the gate ignores
//! (`ServerProtocol`, `SourceIp`, `UserPassword`).

use serde_json::{json, Value};

use crate::harness::{member_of, Emulator};

/// `StatusCode`'s modelled default (`@default(0)`): the value an answer carries when
/// there is no HTTP status from the provider to report.
const NO_RESPONSE_STATUS: i64 = 0;

/// A `ServerId` satisfying the modelled `^s-([0-9a-f]{17})$` that no create minted.
const NEVER_MINTED_SERVER: &str = "s-0000000000000dead";

/// A `UserName` satisfying the modelled `^[\w][\w@.-]{2,99}$`.
const USER: &str = "test.user";

/// A `Function` satisfying `IdentityProviderDetails$Function`'s modelled pattern
/// `^arn:[a-z-]+:lambda:.*$` within `len 1..170`.
const FUNCTION: &str = "arn:aws:lambda:us-east-1:000000000000:function:transfer-idp";

/// A `DirectoryId` satisfying the member's modelled `^d-[0-9a-f]{10}$`.
const DIRECTORY: &str = "d-0123456789";

/// One row: a server configuration, and the endpoint the rule says the test must report
/// for it.
struct Row {
    case: &'static str,
    /// The `CreateServer` members that configure the identity provider.
    configuration: fn() -> Value,
    /// The `Url` the answer must carry: the endpoint the server names, or the empty
    /// string for a server that names none (`Url` is modelled `len 0..255`).
    expected_url: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "an API_GATEWAY server, its API named",
        configuration: || {
            json!({"IdentityProviderType": "API_GATEWAY",
                   "IdentityProviderDetails": {
                       "Url": "https://abc123.execute-api.us-east-1.amazonaws.com/prod"
                   }})
        },
        expected_url: "https://abc123.execute-api.us-east-1.amazonaws.com/prod",
    },
    Row {
        case: "an AWS_LAMBDA server, its function named",
        configuration: || {
            json!({"IdentityProviderType": "AWS_LAMBDA",
                   "IdentityProviderDetails": {"Function": FUNCTION}})
        },
        expected_url: FUNCTION,
    },
    Row {
        case: "an AWS_DIRECTORY_SERVICE server, its directory named",
        configuration: || {
            json!({"IdentityProviderType": "AWS_DIRECTORY_SERVICE",
                   "IdentityProviderDetails": {"DirectoryId": DIRECTORY}})
        },
        expected_url: DIRECTORY,
    },
    Row {
        case: "an AWS_LAMBDA server whose details name no function",
        configuration: || {
            json!({"IdentityProviderType": "AWS_LAMBDA",
                   "IdentityProviderDetails": {"SftpAuthenticationMethods": "PASSWORD"}})
        },
        expected_url: "",
    },
    Row {
        case: "an API_GATEWAY server whose details name no URL",
        configuration: || {
            json!({"IdentityProviderType": "API_GATEWAY",
                   "IdentityProviderDetails": {
                       "SftpAuthenticationMethods": "PUBLIC_KEY_OR_PASSWORD"
                   }})
        },
        expected_url: "",
    },
];

/// RULE 2 (see the module docs): a server that delegates authentication is answered with
/// the endpoint it names, the default status for an answer with no HTTP status in it, no
/// response body and a message saying why.
#[tokio::test]
async fn testing_an_external_identity_provider_reports_what_the_server_names() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let server_id = emulator.create_server((row.configuration)()).await;

        // The request members the gate ignores: each extra one must not change the
        // answer, so the rows are crossed with them.
        let variants: Vec<(&str, Value)> = vec![
            ("the required members alone", json!({})),
            (
                "a password and a protocol",
                json!({"UserPassword": "not-a-real-password", "ServerProtocol": "SFTP"}),
            ),
            (
                "a source IP",
                json!({"SourceIp": "10.0.0.1", "ServerProtocol": "FTP"}),
            ),
        ];
        for (variant, extra) in variants {
            let case = format!("{case} with {variant}");
            let mut request = json!({"ServerId": &server_id, "UserName": USER});
            for (member, value) in extra.as_object().expect("the variant is an object") {
                request[member] = value.clone();
            }
            let answer = emulator.call("TestIdentityProvider", request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected TestIdentityProvider 200, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.get("Url").as_str(),
                Some(row.expected_url),
                "case {case:?}: expected the endpoint the server names ({:?}), got {}",
                row.expected_url,
                answer.body
            );
            assert_eq!(
                answer.get("StatusCode").as_i64(),
                Some(NO_RESPONSE_STATUS),
                "case {case:?}: expected the modelled default status for an answer with no \
                 HTTP status in it, got {}",
                answer.body
            );
            assert!(
                answer.get("Response").is_null(),
                "case {case:?}: expected no Response for a test that got none, got {}",
                answer.body
            );
            let message = answer
                .get("Message")
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {}", answer.body))
                .to_string();
            assert!(
                !message.is_empty(),
                "case {case:?}: expected a message saying whether the test succeeded, got \
                 {message:?}"
            );
            // The text is fixed for the CONDITION: nothing the caller sent, and no
            // identifier of the server, reaches it.
            for sent in [server_id.as_str(), USER, "not-a-real-password", "10.0.0.1"] {
                assert!(
                    !message.contains(sent),
                    "case {case:?}: expected the message to carry no request data, got \
                     {message:?} carrying {sent:?}"
                );
            }
        }
    }
}

/// RULE 1: the gate is the server's own type — and the server is resolved first.
#[tokio::test]
async fn testing_a_server_that_delegates_nothing_is_refused() {
    let emulator = Emulator::new();

    // Each row is a server whose `IdentityProviderType` delegates authentication to
    // nothing: one that names it, and one that leaves it to the documented default.
    for (case, request) in [
        (
            "an explicitly SERVICE_MANAGED server",
            json!({"IdentityProviderType": "SERVICE_MANAGED", "EndpointType": "PUBLIC",
                   "Protocols": ["SFTP"]}),
        ),
        ("a server that names no identity provider type", json!({})),
    ] {
        let server_id = emulator.create_server(request).await;
        let answer = emulator
            .call(
                "TestIdentityProvider",
                json!({"ServerId": &server_id, "UserName": USER}),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} with body {}",
            answer.status,
            answer.body
        );
        // The one documented message of this refusal names the server the request gave.
        assert_eq!(
            member_of(&answer.body, "Message").as_str(),
            Some(format!("{server_id} not configured for external auth").as_str()),
            "case {case:?}: expected the documented refusal text, got {}",
            answer.body
        );
    }

    for (case, absent) in [
        ("a server never created", NEVER_MINTED_SERVER),
        ("another server never created", "s-fffffffffffffffff"),
    ] {
        let answer = emulator
            .call(
                "TestIdentityProvider",
                json!({"ServerId": absent, "UserName": USER}),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected 400 ResourceNotFoundException, got {} with body {}",
            answer.status,
            answer.body
        );
    }
}

/// RULE: a test REPORTS, it does not change the server — two tests answer the same thing
/// and the server is left exactly as it was.
#[tokio::test]
async fn a_test_leaves_the_server_as_it_found_it() {
    let emulator = Emulator::new();
    let server_id = emulator
        .create_server(json!({"IdentityProviderType": "AWS_LAMBDA",
                              "IdentityProviderDetails": {"Function": FUNCTION}}))
        .await;
    let before = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await
        .get("Server")
        .clone();

    let first = emulator
        .call(
            "TestIdentityProvider",
            json!({"ServerId": &server_id, "UserName": USER}),
        )
        .await;
    let second = emulator
        .call(
            "TestIdentityProvider",
            json!({"ServerId": &server_id, "UserName": USER}),
        )
        .await;
    assert_eq!(
        first.body, second.body,
        "expected two tests of one server to answer the same thing, got {} then {}",
        first.body, second.body
    );

    let after = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await
        .get("Server")
        .clone();
    assert_eq!(
        member_of(&before, "IdentityProviderDetails"),
        member_of(&after, "IdentityProviderDetails"),
        "expected a test to leave the server's identity provider untouched, got {after}"
    );
}
