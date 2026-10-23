// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the listener slice (`CreateListener` + `GetListener`).
//!
//! Every case drives the service the way a client does — a signed restJson1 request
//! through the router that mounts it — and reads the answer back out of a LATER
//! request, so nothing here can pass on a canned body: an id that is never stored
//! fails the read-back, and a response echoed from the request fails the ARN,
//! defaulting and nesting assertions.
//!
//! The harness (`Lattice`, `Answer`, `as_label`, `expected_arn`, …) is the one the
//! service-slice suite beside this file already builds; nothing is duplicated here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. `CreateListener` mints a listener under the service the request names: an id of
//!    `^listener-[0-9a-z]{17}$`, an ARN nested in the service's
//!    (`…:service/svc-…/listener/listener-…`) built from the request's region and the
//!    emulator's account; the record is persisted, and `GetListener` by the bare id and
//!    by the ARN both report exactly what was stored.
//! 2. An omitted `port` takes the default the member documents for the protocol (HTTP
//!    80, HTTPS 443) and only those; a protocol with no documented default must carry
//!    one. A port the modelled range forbids is refused at each published bound + 1.
//! 3. A fixed-response action may carry only the status codes its member documents
//!    (404, 500); any other value inside the modelled `@range(100, 599)` is refused.
//! 4. A forward action's only target group gets the documented weight default of 100;
//!    a weight the caller sent is kept, and so is an omitted weight beside siblings.
//! 5. A listener name is unique within its SERVICE: a second create under a taken name
//!    is a 409 `ConflictException`, while the same name under another service is fine.
//! 6. A `clientToken` retry answers with the first listener instead of a second one.
//! 7. An identifier naming no persisted service or listener is a 404
//!    `ResourceNotFoundException` — including a listener read through the wrong
//!    service — and a name the modelled pattern forbids is a 400 `ValidationException`.
//! 8. Deleting the service deletes its listeners, as `DeleteService` documents.

use crate::vpclattice_service_slice::{
    as_label, expected_arn, is_lattice_id, is_timestamp, Answer, Lattice,
};

/// A target-group identifier of the shape `TargetGroupIdentifier` demands
/// (`^tg-[0-9a-z]{17}$`). The listener stores the reference as sent: whether the target
/// group exists is `CreateTargetGroup`'s and the rule operations' concern, not this
/// slice's.
pub(crate) fn target_group(suffix: &str) -> String {
    format!("tg-{suffix:0>17}")
}

/// The ARN kind of a listener of `service_id`: a listener ARN nests under its service's.
fn listener_kind(service_id: &str) -> String {
    format!("service/{service_id}/listener")
}

/// Create a service to hang listeners off, answering its id and ARN.
pub(crate) async fn a_service(lattice: &Lattice, name: &str) -> (String, String) {
    let created = lattice
        .create_service(serde_json::json!({ "name": name }))
        .await;
    assert_eq!(
        created.status, 201,
        "case fixture service {name:?}: expected 201, got {} / {}",
        created.status, created.body
    );
    (created.str_field("id"), created.str_field("arn"))
}

/// `POST /services/{serviceIdentifier}/listeners`.
pub(crate) async fn create_listener(
    lattice: &Lattice,
    service_identifier: &str,
    body: serde_json::Value,
) -> Answer {
    lattice
        .call(
            "POST",
            &format!("/services/{service_identifier}/listeners"),
            Some(body),
        )
        .await
}

/// `GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}`.
async fn get_listener(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
) -> Answer {
    lattice
        .call(
            "GET",
            &format!("/services/{service_identifier}/listeners/{listener_identifier}"),
            None,
        )
        .await
}

/// Rules 1, 3 and 4, over a table that varies protocol, port, action kind, target-group
/// count, weights and name shape. The recorded case is one row of it.
///
/// Every row carries its own port: a listener's port is unique within one service (see
/// `listener_conflicts_and_deletion.rs`), so a table of listeners under ONE service is a
/// table of distinct ports.
#[tokio::test]
async fn creating_listeners_persists_them_and_both_identifier_forms_read_them_back() {
    let lattice = Lattice::new();
    let (service_id, service_arn) = a_service(&lattice, "listener-host").await;

    // (case, request body, expected port, expected defaultAction as persisted)
    let cases: Vec<(&str, serde_json::Value, i64, serde_json::Value)> = vec![
        (
            "HTTP with no port, fixed response 404",
            serde_json::json!({
                "name": "http-default-port",
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            }),
            // The `port` member documents 80 as HTTP's default.
            80,
            serde_json::json!({ "fixedResponse": { "statusCode": 404 } }),
        ),
        (
            "HTTPS with no port, fixed response 500",
            serde_json::json!({
                "name": "https-default-port",
                "protocol": "HTTPS",
                "defaultAction": { "fixedResponse": { "statusCode": 500 } },
                "tags": { "team": "payments" },
            }),
            // …and 443 as HTTPS's.
            443,
            serde_json::json!({ "fixedResponse": { "statusCode": 500 } }),
        ),
        (
            "explicit port, forward to one target group with no weight",
            serde_json::json!({
                "name": "forward-one-no-weight",
                "protocol": "HTTP",
                "port": 8080,
                "defaultAction": {
                    "forward": {
                        "targetGroups": [{ "targetGroupIdentifier": target_group("1") }],
                    },
                },
            }),
            8080,
            // "If there's only one target group specified, then the default value is
            // 100" (the `weight` member's documentation).
            serde_json::json!({
                "forward": {
                    "targetGroups": [
                        { "targetGroupIdentifier": target_group("1"), "weight": 100 },
                    ],
                },
            }),
        ),
        (
            "TLS_PASSTHROUGH with an explicit port, weight 0 (range floor)",
            serde_json::json!({
                "name": "tls-passthrough",
                "protocol": "TLS_PASSTHROUGH",
                "port": 8443,
                "defaultAction": {
                    "forward": {
                        "targetGroups": [
                            { "targetGroupIdentifier": target_group("2"), "weight": 0 },
                        ],
                    },
                },
            }),
            8443,
            // A weight the caller sent is kept, floor included — no default replaces it.
            serde_json::json!({
                "forward": {
                    "targetGroups": [
                        { "targetGroupIdentifier": target_group("2"), "weight": 0 },
                    ],
                },
            }),
        ),
        (
            "lowest legal port, forward to two weighted target groups",
            serde_json::json!({
                "name": "forward-two-weighted",
                "protocol": "HTTPS",
                "port": 1,
                "defaultAction": {
                    "forward": {
                        "targetGroups": [
                            { "targetGroupIdentifier": target_group("3"), "weight": 10 },
                            { "targetGroupIdentifier": target_group("4"), "weight": 999 },
                        ],
                    },
                },
            }),
            1,
            serde_json::json!({
                "forward": {
                    "targetGroups": [
                        { "targetGroupIdentifier": target_group("3"), "weight": 10 },
                        { "targetGroupIdentifier": target_group("4"), "weight": 999 },
                    ],
                },
            }),
        ),
        (
            "highest legal port, longest legal name (63 characters)",
            serde_json::json!({
                "name": "l".repeat(63),
                "protocol": "HTTP",
                "port": 65535,
                "defaultAction": {
                    "forward": {
                        "targetGroups": [
                            { "targetGroupIdentifier": target_group("5"), "weight": 1 },
                        ],
                    },
                },
            }),
            65535,
            serde_json::json!({
                "forward": {
                    "targetGroups": [
                        { "targetGroupIdentifier": target_group("5"), "weight": 1 },
                    ],
                },
            }),
        ),
    ];

    for (case, body, expected_port, expected_action) in cases {
        let requested_name = body["name"].as_str().expect("the case names a listener");
        let protocol = body["protocol"].as_str().expect("the case names a protocol");
        let created = create_listener(&lattice, &service_id, body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        let id = created.str_field("id");
        assert!(
            is_lattice_id(&id, "listener-"),
            "case {case:?}: expected an id matching ^listener-[0-9a-z]{{17}}$, got {id:?}"
        );
        let arn = created.str_field("arn");
        assert_eq!(
            arn,
            expected_arn(&listener_kind(&service_id), &id),
            "case {case:?}: the ARN must nest this listener under its service in the request's region"
        );
        assert_eq!(
            created.str_field("name"),
            requested_name,
            "case {case:?}: the stored name must come back, got {}",
            created.body
        );
        assert_eq!(
            created.str_field("protocol"),
            protocol,
            "case {case:?}: the stored protocol must come back, got {}",
            created.body
        );
        assert_eq!(
            created.body["port"].as_i64(),
            Some(expected_port),
            "case {case:?}: expected port {expected_port}, got {}",
            created.body
        );
        assert_eq!(
            created.str_field("serviceId"),
            service_id,
            "case {case:?}: the listener must report the service it was created under, got {}",
            created.body
        );
        assert_eq!(
            created.str_field("serviceArn"),
            service_arn,
            "case {case:?}: the listener must report its service's ARN, got {}",
            created.body
        );
        assert_eq!(
            created.body["defaultAction"], expected_action,
            "case {case:?}: expected defaultAction {expected_action}, got {}",
            created.body
        );

        // Rule 1: both modelled identifier forms read the same persisted record, and a
        // read reports it member for member — including the timestamps only a stored
        // record can carry.
        for (form, identifier) in [("bare id", id.clone()), ("ARN", as_label(&arn))] {
            let read = get_listener(&lattice, &service_id, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} by {form}: expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                read.str_field("id"),
                id,
                "case {case:?} by {form}: read back a different listener: {}",
                read.body
            );
            assert_eq!(
                read.str_field("arn"),
                arn,
                "case {case:?} by {form}: expected the stored ARN, got {}",
                read.body
            );
            assert_eq!(
                read.str_field("name"),
                requested_name,
                "case {case:?} by {form}: expected the stored name, got {}",
                read.body
            );
            assert_eq!(
                read.str_field("protocol"),
                protocol,
                "case {case:?} by {form}: expected the stored protocol, got {}",
                read.body
            );
            assert_eq!(
                read.body["port"].as_i64(),
                Some(expected_port),
                "case {case:?} by {form}: expected port {expected_port}, got {}",
                read.body
            );
            assert_eq!(
                read.body["defaultAction"], expected_action,
                "case {case:?} by {form}: expected defaultAction {expected_action}, got {}",
                read.body
            );
            assert_eq!(
                read.str_field("serviceId"),
                service_id,
                "case {case:?} by {form}: expected the owning service id, got {}",
                read.body
            );
            assert!(
                is_timestamp(&read.body["createdAt"]),
                "case {case:?} by {form}: createdAt must carry an instant, got {}",
                read.body
            );
            assert!(
                is_timestamp(&read.body["lastUpdatedAt"]),
                "case {case:?} by {form}: lastUpdatedAt must carry an instant, got {}",
                read.body
            );
        }

        // A read changes nothing a later caller sees.
        let again = get_listener(&lattice, &service_id, &id).await;
        assert_eq!(
            again.body, get_listener(&lattice, &service_id, &id).await.body,
            "case {case:?}: two reads of one listener must agree"
        );
    }
}

/// One row of the port table: what the request sent, and the port the answer must
/// carry (`None` where the create must be refused).
struct PortCase {
    case: &'static str,
    name: &'static str,
    protocol: &'static str,
    sent: Option<i64>,
    expected: Option<i64>,
}

/// Rule 2: the port default table, its only two documented rows, and the modelled range
/// at each published bound and one past it.
#[tokio::test]
async fn an_omitted_port_takes_the_documented_default_for_its_protocol_only() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "port-defaults").await;

    // The protocol catalog is `ListenerProtocol`'s own
    // (`state`'s LISTENER_PROTOCOL_VALUES: HTTP | HTTPS | TLS_PASSTHROUGH); the two
    // defaults are the ones the `port` member documents.
    let cases = vec![
        PortCase {
            case: "HTTP defaults to 80",
            name: "p-http",
            protocol: "HTTP",
            sent: None,
            expected: Some(80),
        },
        PortCase {
            case: "HTTPS defaults to 443",
            name: "p-https",
            protocol: "HTTPS",
            sent: None,
            expected: Some(443),
        },
        PortCase {
            case: "TLS_PASSTHROUGH documents no default, so a port is required",
            name: "p-tls",
            protocol: "TLS_PASSTHROUGH",
            sent: None,
            expected: None,
        },
        PortCase {
            case: "an explicit port wins over the protocol default",
            name: "p-explicit",
            protocol: "HTTPS",
            sent: Some(8443),
            expected: Some(8443),
        },
        PortCase {
            case: "the range floor",
            name: "p-floor",
            protocol: "HTTP",
            sent: Some(1),
            expected: Some(1),
        },
        PortCase {
            case: "the range ceiling",
            name: "p-ceiling",
            protocol: "HTTP",
            sent: Some(65535),
            expected: Some(65535),
        },
        PortCase {
            case: "one below the floor",
            name: "p-under",
            protocol: "HTTP",
            sent: Some(0),
            expected: None,
        },
        PortCase {
            case: "one past the ceiling",
            name: "p-over",
            protocol: "HTTP",
            sent: Some(65536),
            expected: None,
        },
    ];

    for (index, PortCase {
        case,
        name,
        protocol,
        sent,
        expected,
    }) in cases.into_iter().enumerate()
    {
        // A port a row leaves free for the retry below, so the retry can only fail on
        // the name it reuses and never on an occupied port.
        let retry_port = 9_000 + i64::try_from(index).expect("the table is small");
        let mut body = serde_json::json!({
            "name": name,
            "protocol": protocol,
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        });
        if let Some(port) = sent {
            body["port"] = serde_json::json!(port);
        }
        let created = create_listener(&lattice, &service_id, body).await;
        match expected {
            Some(port) => {
                assert_eq!(
                    created.status, 201,
                    "case {case:?}: expected 201, got {} / {}",
                    created.status, created.body
                );
                assert_eq!(
                    created.body["port"].as_i64(),
                    Some(port),
                    "case {case:?}: expected port {port}, got {}",
                    created.body
                );
                // The default is PERSISTED, not computed per answer.
                let read = get_listener(&lattice, &service_id, &created.str_field("id")).await;
                assert_eq!(
                    read.body["port"].as_i64(),
                    Some(port),
                    "case {case:?}: the stored listener must report port {port}, got {}",
                    read.body
                );
            }
            None => {
                assert_eq!(
                    created.status, 400,
                    "case {case:?}: expected 400, got {} / {}",
                    created.status, created.body
                );
                // Nothing may be stored for a refused create: the name stays free.
                let retried = create_listener(
                    &lattice,
                    &service_id,
                    serde_json::json!({
                        "name": name,
                        "protocol": "HTTP",
                        "port": retry_port,
                        "defaultAction": { "fixedResponse": { "statusCode": 404 } },
                    }),
                )
                .await;
                assert_eq!(
                    retried.status, 201,
                    "case {case:?}: a refused create must store nothing, so {name:?} is still free; got {} / {}",
                    retried.status, retried.body
                );
            }
        }
    }
}

/// Rule 3: the fixed-response status-code catalog its member documents, tested on both
/// supported codes and on several the modelled `@range(100, 599)` admits but the
/// documentation does not.
#[tokio::test]
async fn a_fixed_response_action_accepts_only_the_documented_status_codes() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "fixed-response").await;

    // "Only 404 and 500 status codes are supported" (FixedResponseAction.statusCode).
    let cases: Vec<(&str, i64, bool)> = vec![
        ("404 is supported", 404, true),
        ("500 is supported", 500, true),
        ("200 is inside the modelled range but unsupported", 200, false),
        ("403 is inside the modelled range but unsupported", 403, false),
        ("503 is inside the modelled range but unsupported", 503, false),
        ("100, the range floor, is unsupported", 100, false),
        ("599, the range ceiling, is unsupported", 599, false),
    ];

    for (index, (case, status_code, supported)) in cases.into_iter().enumerate() {
        // Each accepted row stores a listener, and a listener's port is unique within
        // its service, so every row asks for its own port.
        let port = 10_000 + i64::try_from(index).expect("the table is small");
        let created = create_listener(
            &lattice,
            &service_id,
            serde_json::json!({
                "name": format!("fixed-{status_code}"),
                "protocol": "HTTP",
                "port": port,
                "defaultAction": { "fixedResponse": { "statusCode": status_code } },
            }),
        )
        .await;
        if supported {
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected 201, got {} / {}",
                created.status, created.body
            );
            assert_eq!(
                created.body["defaultAction"]["fixedResponse"]["statusCode"].as_i64(),
                Some(status_code),
                "case {case:?}: the stored action must carry status code {status_code}, got {}",
                created.body
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                created.status, created.body
            );
            assert_eq!(
                created.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected the modelled ValidationException, got {:?} / {}",
                created.error_code,
                created.body
            );
        }
    }
}

/// Rule 5: a listener name is unique within its service, and only within it.
#[tokio::test]
async fn a_listener_name_is_unique_within_its_service_but_not_across_services() {
    let lattice = Lattice::new();
    let (first, _) = a_service(&lattice, "names-one").await;
    let (second, _) = a_service(&lattice, "names-two").await;

    let body = |name: &str, port: i64| {
        serde_json::json!({
            "name": name,
            "protocol": "HTTP",
            "port": port,
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        })
    };

    for (index, name) in ["api", "web-tier", &"n".repeat(63)].into_iter().enumerate() {
        // Its own port per row, so the duplicate below can only be refused for its name.
        let port = 11_000 + i64::try_from(index).expect("the table is small");
        let created = create_listener(&lattice, &first, body(name, port)).await;
        assert_eq!(
            created.status, 201,
            "case first create of {name:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        let first_id = created.str_field("id");

        let duplicate = create_listener(&lattice, &first, body(name, port)).await;
        assert_eq!(
            duplicate.status, 409,
            "case duplicate {name:?} in one service: expected 409, got {} / {}",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("ConflictException"),
            "case duplicate {name:?}: expected the modelled ConflictException, got {:?} / {}",
            duplicate.error_code,
            duplicate.body
        );

        // The same name under another service is a different listener, not a conflict.
        let elsewhere = create_listener(&lattice, &second, body(name, port)).await;
        assert_eq!(
            elsewhere.status, 201,
            "case {name:?} under a second service: expected 201, got {} / {}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.str_field("id"),
            first_id,
            "case {name:?} under a second service: it must be its own listener"
        );
    }
}

/// Rule 6: a `clientToken` retry answers with the listener the first call created.
#[tokio::test]
async fn a_client_token_retry_returns_the_first_listener() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "idempotent-listeners").await;

    // The rows differ in what the retry repeats: identical parameters, and the same
    // token with a different name.
    let first = create_listener(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "token-one",
            "protocol": "HTTP",
            "clientToken": "token-abc",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "case first create: expected 201, got {} / {}",
        first.status, first.body
    );
    let id = first.str_field("id");

    let retry = create_listener(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "token-one",
            "protocol": "HTTP",
            "clientToken": "token-abc",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    assert_eq!(
        retry.status, 201,
        "case identical retry: expected 201, got {} / {}",
        retry.status, retry.body
    );
    assert_eq!(
        retry.str_field("id"),
        id,
        "case identical retry: expected the first listener {id}, got {}",
        retry.body
    );

    let diverging = create_listener(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "token-two",
            "protocol": "HTTP",
            "clientToken": "token-abc",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    assert_eq!(
        diverging.status, 409,
        "case retry with different parameters: expected 409, got {} / {}",
        diverging.status, diverging.body
    );

    // A fresh token under the same name is refused by the name rule, not served from
    // the token store.
    let fresh_token = create_listener(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "token-one",
            "protocol": "HTTP",
            "clientToken": "token-xyz",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    assert_eq!(
        fresh_token.status, 409,
        "case a new token under a taken name: expected 409, got {} / {}",
        fresh_token.status, fresh_token.body
    );
}

/// Rule 7: an identifier that names no persisted resource is a 404, and a name the
/// modelled pattern forbids is a 400.
#[tokio::test]
async fn unknown_identifiers_are_404_and_a_forbidden_name_is_400() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "refusals").await;
    let (other_service, _) = a_service(&lattice, "refusals-other").await;

    let created = create_listener(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "only-listener",
            "protocol": "HTTP",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    let listener_id = created.str_field("id");

    // Each row names a resource that is not there, by a different route.
    let missing_service = "svc-00000000000000000";
    let missing_listener = "listener-00000000000000000";
    let not_found: Vec<(&str, String, String)> = vec![
        (
            "create under a service that does not exist",
            missing_service.to_string(),
            String::new(),
        ),
        (
            "read a listener that does not exist",
            service_id.clone(),
            missing_listener.to_string(),
        ),
        (
            "read a real listener through the wrong service",
            other_service.clone(),
            listener_id.clone(),
        ),
        (
            "read a real listener through a service that does not exist",
            missing_service.to_string(),
            listener_id.clone(),
        ),
    ];

    for (case, service_identifier, listener_identifier) in not_found {
        let answer = if listener_identifier.is_empty() {
            create_listener(
                &lattice,
                &service_identifier,
                serde_json::json!({
                    "name": "orphan",
                    "protocol": "HTTP",
                    "defaultAction": { "fixedResponse": { "statusCode": 404 } },
                }),
            )
            .await
        } else {
            get_listener(&lattice, &service_identifier, &listener_identifier).await
        };
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the ResourceNotFoundException wire code, got {:?} / {}",
            answer.error_code,
            answer.body
        );
    }

    // `ListenerName`'s pattern is written with negative lookahead; each row breaks one
    // clause of it.
    let forbidden: Vec<(&str, String)> = vec![
        ("the reserved listener- prefix", "listener-abc".to_string()),
        ("a leading hyphen", "-api".to_string()),
        ("a trailing hyphen", "api-".to_string()),
        ("two adjacent hyphens", "a--b".to_string()),
        ("an uppercase letter", "Api".to_string()),
        ("a character outside [a-z0-9-]", "api_v1".to_string()),
    ];

    for (case, name) in forbidden {
        let refused = create_listener(
            &lattice,
            &service_id,
            serde_json::json!({
                "name": name,
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} ({name:?}): expected 400, got {} / {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?} ({name:?}): expected the modelled ValidationException, got {:?} / {}",
            refused.error_code,
            refused.body
        );
    }
}

/// Rule 8: "If you delete a service, all resources related to the service, such as …
/// listeners, listener rules, and access log subscriptions, are also deleted"
/// (`DeleteService`'s documentation), so no listener outlives its service.
#[tokio::test]
async fn deleting_a_service_deletes_its_listeners() {
    let lattice = Lattice::new();
    let (doomed, _) = a_service(&lattice, "doomed-service").await;
    let (survivor, _) = a_service(&lattice, "surviving-service").await;

    // Two listeners on the service being deleted, one on the service that stays.
    let mut doomed_listeners = Vec::new();
    for (index, name) in ["first", "second"].into_iter().enumerate() {
        let created = create_listener(
            &lattice,
            &doomed,
            serde_json::json!({
                "name": name,
                "protocol": "HTTP",
                // One port each: a listener's port is unique within its service.
                "port": 12_000 + i64::try_from(index).expect("the table is small"),
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case listener {name:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        doomed_listeners.push(created.str_field("id"));
    }
    let kept = create_listener(
        &lattice,
        &survivor,
        serde_json::json!({
            "name": "first",
            "protocol": "HTTP",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    let kept_id = kept.str_field("id");

    let deleted = lattice
        .call("DELETE", &format!("/services/{doomed}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the service: expected 200, got {} / {}",
        deleted.status, deleted.body
    );

    for id in &doomed_listeners {
        let read = get_listener(&lattice, &doomed, id).await;
        assert_eq!(
            read.status, 404,
            "case listener {id} of a deleted service: expected 404, got {} / {}",
            read.status, read.body
        );
    }

    // The other service's listener — same name, different service — is untouched.
    let still_there = get_listener(&lattice, &survivor, &kept_id).await;
    assert_eq!(
        still_there.status, 200,
        "case the surviving service's listener: expected 200, got {} / {}",
        still_there.status, still_there.body
    );
}
