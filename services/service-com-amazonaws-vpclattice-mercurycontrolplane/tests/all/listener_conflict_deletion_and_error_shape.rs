// SPDX-License-Identifier: CC0-1.0

//! The listener invariants and error CONSTRUCTIONS of the `CreateListener` /
//! `GetListener` / `DeleteListener` path, each asserted over a TABLE of inputs.
//!
//! The rules under test:
//!
//! 1. A listener's PORT is unique within one service: a create whose effective port
//!    (explicit, or the default its protocol documents) is already bound by a listener of
//!    that service is a 409 `ConflictException` naming the listener that holds it, while
//!    the same port under another service, a free port, and a port released by a delete
//!    are all accepted.
//! 2. A delete REMOVES the listener: it answers the modelled 204, every later read of
//!    that identifier is the modelled 404, the listener's rules go with it, and the name
//!    and port it held are free again. A delete that resolves nothing changes nothing.
//! 3. A value a documented SERVICE RULE refuses — one no modelled constraint carries —
//!    is reported with the rule's own sentence over the BARE member the rule constrains:
//!    `fieldList` holds one entry whose `name` is that member and whose `message` is the
//!    sentence, and the top-level `message` IS that sentence. The
//!    `"<N> validation error(s) detected: Value '…' at '<path>' failed to satisfy
//!    constraint: …"` envelope stays what it describes: a MODELLED constraint violation.
//! 4. A duplicate-name conflict names the kind whose name is taken, the submitted name
//!    and the scope the name must be unique in — the owning resource for a name scoped to
//!    one (a listener in its service, a rule in its listener).
//!
//! Every row drives the service over HTTP the way a client does and reads the outcome
//! back out of a LATER request, so no assertion here can pass on a canned body.

use crate::listeners::{a_service, create_listener};
use crate::vpclattice_service_slice::{as_label, message_of, Answer, Lattice};

/// The resource-type token AWS's listener errors carry, written out here rather than read
/// from the code under test.
const LISTENER_TYPE: &str = "LISTENER";
/// The same for a listener rule.
const RULE_TYPE: &str = "RULE";

/// `GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}`.
async fn get_listener(lattice: &Lattice, service: &str, listener: &str) -> Answer {
    lattice
        .call("GET", &format!("/services/{service}/listeners/{listener}"), None)
        .await
}

/// `DELETE /services/{serviceIdentifier}/listeners/{listenerIdentifier}`.
async fn delete_listener(lattice: &Lattice, service: &str, listener: &str) -> Answer {
    lattice
        .call(
            "DELETE",
            &format!("/services/{service}/listeners/{listener}"),
            None,
        )
        .await
}

/// `POST /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules`.
async fn create_rule(
    lattice: &Lattice,
    service: &str,
    listener: &str,
    body: serde_json::Value,
) -> Answer {
    lattice
        .call(
            "POST",
            &format!("/services/{service}/listeners/{listener}/rules"),
            Some(body),
        )
        .await
}

/// `GET /services/{…}/listeners/{…}/rules/{ruleIdentifier}`.
async fn get_rule(lattice: &Lattice, service: &str, listener: &str, rule: &str) -> Answer {
    lattice
        .call(
            "GET",
            &format!("/services/{service}/listeners/{listener}/rules/{rule}"),
            None,
        )
        .await
}

/// A listener create body: the name, the protocol, and the port only when the row sends
/// one (an omitted port exercises the protocol's documented default).
fn listener_body(name: &str, protocol: &str, port: Option<i64>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": name,
        "protocol": protocol,
        "defaultAction": { "fixedResponse": { "statusCode": 404 } },
    });
    if let Some(port) = port {
        body["port"] = serde_json::json!(port);
    }
    body
}

/// A rule create body with a fixed-response action, for the rows where only the name or
/// the status code is under test.
fn rule_body(name: &str, priority: i64, status_code: i64) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "priority": priority,
        "match": { "httpMatch": { "pathMatch": { "match": { "prefix": "/" } } } },
        "action": { "fixedResponse": { "statusCode": status_code } },
    })
}

/// A 409 of the shape every `ConflictException` of this service must have: the status,
/// the wire error code, the kind's resource-type token and the id of the resource the
/// request collided with.
fn assert_conflict(answer: &Answer, case: &str, resource_type: &str, resource_id: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected 409, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ConflictException"),
        "case {case:?}: expected the ConflictException wire code, got {:?} / {}",
        answer.error_code,
        answer.body
    );
    assert_eq!(
        answer.body["resourceType"].as_str(),
        Some(resource_type),
        "case {case:?}: expected resourceType {resource_type}, got {}",
        answer.body
    );
    assert_eq!(
        answer.body["resourceId"].as_str(),
        Some(resource_id),
        "case {case:?}: resourceId must name the resource the request collided with, got {}",
        answer.body
    );
}

/// One row of the port table: how the two creates ask for the SAME effective port.
struct PortRow {
    case: &'static str,
    /// The protocol and port the listener that takes the port is created with.
    first: (&'static str, Option<i64>),
    /// The protocol and port the second create asks for — a different listener NAME, the
    /// same effective port.
    second: (&'static str, Option<i64>),
    /// The effective port both rows land on, which the conflict must report.
    port: i64,
}

/// Rule 1: the effective port of a listener is unique within its service.
#[tokio::test]
async fn a_listener_port_is_unique_within_its_service() {
    let lattice = Lattice::new();

    // The rows differ along what the rule generalises over: how each side names the port
    // (explicit or the protocol's documented default), the protocols involved (the port
    // is unique per SERVICE, not per protocol), and where the port sits in the modelled
    // `Port` range (floor, ceiling, in between). The recorded case — HTTP 80 twice — is
    // the first row.
    let rows = vec![
        PortRow {
            case: "both sides take HTTP's documented default of 80",
            first: ("HTTP", None),
            second: ("HTTP", None),
            port: 80,
        },
        PortRow {
            case: "a default port is occupied by an explicit one",
            first: ("HTTP", None),
            second: ("HTTP", Some(80)),
            port: 80,
        },
        PortRow {
            case: "an explicit port is occupied by HTTPS's documented default",
            first: ("HTTPS", Some(443)),
            second: ("HTTPS", None),
            port: 443,
        },
        PortRow {
            case: "the same explicit port under two different protocols",
            first: ("HTTP", Some(8081)),
            second: ("TLS_PASSTHROUGH", Some(8081)),
            port: 8081,
        },
        PortRow {
            case: "the range floor",
            first: ("HTTP", Some(1)),
            second: ("HTTPS", Some(1)),
            port: 1,
        },
        PortRow {
            case: "the range ceiling",
            first: ("HTTPS", Some(65535)),
            second: ("HTTP", Some(65535)),
            port: 65535,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let PortRow {
            case,
            first,
            second,
            port,
        } = row;
        // One service per row, so a row's ports are never occupied by another's.
        let (service_id, _) = a_service(&lattice, &format!("port-rule-{index}")).await;
        let (elsewhere_id, _) = a_service(&lattice, &format!("port-rule-other-{index}")).await;

        let taken = create_listener(
            &lattice,
            &service_id,
            listener_body("holder", first.0, first.1),
        )
        .await;
        assert_eq!(
            taken.status, 201,
            "case {case:?}: the first create expected 201, got {} / {}",
            taken.status, taken.body
        );
        let holder_id = taken.str_field("id");
        assert_eq!(
            taken.body["port"].as_i64(),
            Some(port),
            "case {case:?}: the first listener must bind port {port}, got {}",
            taken.body
        );

        // A second listener of the SAME service on that port is refused, and the refusal
        // names the listener that holds the port.
        let clash = create_listener(
            &lattice,
            &service_id,
            listener_body("contender", second.0, second.1),
        )
        .await;
        assert_conflict(&clash, case, LISTENER_TYPE, &holder_id);
        let message = message_of(case, &clash);
        assert_eq!(
            message,
            format!("Port {port} is occupied"),
            "case {case:?}: the conflict must name the occupied port, got {message:?}"
        );

        // Nothing was stored for the refused create: the name it asked for is still free
        // (on a port that is free too).
        let free_port = create_listener(
            &lattice,
            &service_id,
            listener_body("contender", second.0, Some(if port == 65535 { 65534 } else { port + 1 })),
        )
        .await;
        assert_eq!(
            free_port.status, 201,
            "case {case:?}: a free port of the same service expected 201, got {} / {}",
            free_port.status, free_port.body
        );

        // The port is unique within ONE service: the same port under another service is
        // a different listener, not a conflict.
        let other_service = create_listener(
            &lattice,
            &elsewhere_id,
            listener_body("holder", second.0, second.1),
        )
        .await;
        assert_eq!(
            other_service.status, 201,
            "case {case:?}: the same port under another service expected 201, got {} / {}",
            other_service.status, other_service.body
        );
        assert_eq!(
            other_service.body["port"].as_i64(),
            Some(port),
            "case {case:?}: the other service's listener must bind port {port}, got {}",
            other_service.body
        );

        // Deleting the holder RELEASES the port, so the refused create now succeeds.
        let deleted = delete_listener(&lattice, &service_id, &holder_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete expected 204, got {} / {}",
            deleted.status, deleted.body
        );
        let reused = create_listener(
            &lattice,
            &service_id,
            listener_body("successor", second.0, second.1),
        )
        .await;
        assert_eq!(
            reused.status, 201,
            "case {case:?}: a port released by a delete expected 201, got {} / {}",
            reused.status, reused.body
        );
        assert_eq!(
            reused.body["port"].as_i64(),
            Some(port),
            "case {case:?}: the successor must bind the released port {port}, got {}",
            reused.body
        );
    }
}

/// Rule 2: a deleted listener is gone — from `GetListener`, from the name index, and
/// along with its rules — while a delete that resolves nothing changes nothing.
#[tokio::test]
async fn a_deleted_listener_is_gone_from_every_later_read() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "deletion-host").await;
    let (other_id, _) = a_service(&lattice, "deletion-other").await;

    // The rows differ in the identifier FORM the delete and the read use (both modelled
    // forms), the protocol, and the port (explicit or the protocol's default).
    let rows: Vec<(&str, &str, Option<i64>, bool)> = vec![
        ("delete by bare id, HTTP default port", "HTTP", None, false),
        ("delete by ARN, HTTPS default port", "HTTPS", None, true),
        (
            "delete by bare id, explicit port",
            "TLS_PASSTHROUGH",
            Some(7443),
            false,
        ),
        ("delete by ARN, explicit port", "HTTP", Some(7080), true),
    ];

    for (case, protocol, port, by_arn) in rows {
        let created = create_listener(
            &lattice,
            &service_id,
            listener_body("doomed", protocol, port),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the create expected 201, got {} / {}",
            created.status, created.body
        );
        let id = created.str_field("id");
        let arn = created.str_field("arn");
        let identifier = if by_arn { arn.clone() } else { id.clone() };
        let label = as_label(&identifier);

        // A listener of another service on the same name, to prove the delete removes one
        // listener and not a name.
        let bystander = create_listener(
            &lattice,
            &other_id,
            listener_body("doomed", protocol, port),
        )
        .await;
        assert_eq!(
            bystander.status, 201,
            "case {case:?}: the bystander create expected 201, got {} / {}",
            bystander.status, bystander.body
        );
        let bystander_id = bystander.str_field("id");

        // A rule of the doomed listener: a rule is a resource OF its listener.
        let rule = create_rule(
            &lattice,
            &service_id,
            &id,
            rule_body("doomed-rule", 10, 404),
        )
        .await;
        assert_eq!(
            rule.status, 201,
            "case {case:?}: the rule create expected 201, got {} / {}",
            rule.status, rule.body
        );
        let rule_id = rule.str_field("id");

        // A delete through the WRONG service resolves nothing and removes nothing.
        let wrong_service = delete_listener(&lattice, &other_id, &label).await;
        assert_eq!(
            wrong_service.status, 404,
            "case {case:?}: a delete through the wrong service expected 404, got {} / {}",
            wrong_service.status, wrong_service.body
        );
        assert_eq!(
            get_listener(&lattice, &service_id, &label).await.status,
            200,
            "case {case:?}: a refused delete must leave the listener readable"
        );

        let deleted = delete_listener(&lattice, &service_id, &label).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete expected 204, got {} / {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted
                .body
                .as_object()
                .is_none_or(serde_json::Map::is_empty),
            "case {case:?}: DeleteListenerResponse models no member, so the 204 reports \
             none, got {}",
            deleted.body
        );

        // Every later read of that listener is the modelled 404, by EITHER identifier
        // form, with the message and members AWS's not-found construction carries.
        for (form, read_identifier) in [("bare id", id.clone()), ("ARN", arn.clone())] {
            let read = get_listener(&lattice, &service_id, &as_label(&read_identifier)).await;
            let read_case = format!("{case} read back by {form}");
            assert_eq!(
                read.status, 404,
                "case {read_case:?}: expected 404, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                read.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {read_case:?}: expected the ResourceNotFoundException wire code, got {:?} / {}",
                read.error_code,
                read.body
            );
            let message = message_of(&read_case, &read);
            assert_eq!(
                message,
                format!("Listener {read_identifier} cannot be found."),
                "case {read_case:?}: the message must name the kind and the identifier \
                 sent, got {message:?}"
            );
            assert_eq!(
                read.body["resourceId"].as_str(),
                Some(read_identifier.as_str()),
                "case {read_case:?}: resourceId must be the identifier sent, got {}",
                read.body
            );
            assert_eq!(
                read.body["resourceType"].as_str(),
                Some(LISTENER_TYPE),
                "case {read_case:?}: expected resourceType {LISTENER_TYPE}, got {}",
                read.body
            );
        }

        // The rule went with its listener.
        let orphan = get_rule(&lattice, &service_id, &id, &rule_id).await;
        assert_eq!(
            orphan.status, 404,
            "case {case:?}: a rule of a deleted listener expected 404, got {} / {}",
            orphan.status, orphan.body
        );

        // A second delete of the same identifier reports the same absence.
        let again = delete_listener(&lattice, &service_id, &label).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second delete expected 404, got {} / {}",
            again.status, again.body
        );

        // The name and the port the listener held are free again…
        let successor = create_listener(
            &lattice,
            &service_id,
            listener_body("doomed", protocol, port),
        )
        .await;
        assert_eq!(
            successor.status, 201,
            "case {case:?}: the released name and port expected 201, got {} / {}",
            successor.status, successor.body
        );
        assert_ne!(
            successor.str_field("id"),
            id,
            "case {case:?}: the successor must be its own listener"
        );
        // …and nothing else was touched: the other service's listener is still there.
        assert_eq!(
            get_listener(&lattice, &other_id, &bystander_id).await.status,
            200,
            "case {case:?}: the other service's listener must survive this delete"
        );

        // Clean up this row's listeners so the next row starts from free ports.
        for (service, listener) in [
            (&service_id, successor.str_field("id")),
            (&other_id, bystander_id),
        ] {
            let cleaned = delete_listener(&lattice, service, &listener).await;
            assert_eq!(
                cleaned.status, 204,
                "case {case:?}: cleaning up {listener} expected 204, got {} / {}",
                cleaned.status, cleaned.body
            );
        }
    }
}

/// Rule 3: a documented SERVICE rule is reported over the bare member it constrains,
/// while a MODELLED constraint keeps the framework's constraint-violation construction.
#[tokio::test]
async fn a_service_rule_refusal_names_the_bare_member_it_constrains() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "rule-shape-host").await;
    let host_listener = create_listener(
        &lattice,
        &service_id,
        listener_body("rule-shape-listener", "HTTP", Some(6080)),
    )
    .await;
    assert_eq!(
        host_listener.status, 201,
        "fixture listener: expected 201, got {} / {}",
        host_listener.status, host_listener.body
    );
    let listener_id = host_listener.str_field("id");

    // "The HTTP response code. Only 404 and 500 status codes are supported."
    // (`FixedResponseAction`'s `statusCode` documentation) — a rule no modelled trait
    // carries, so every value inside the modelled `@range(100, 599)` that is not in the
    // catalog is refused, at both published bounds of that range and in between.
    let unsupported: Vec<i64> = vec![100, 200, 403, 503, 599];
    let supported: Vec<i64> = vec![404, 500];

    for (index, status_code) in unsupported.into_iter().enumerate() {
        let port = 6_100 + i64::try_from(index).expect("the table is small");
        let priority = 100 + i64::try_from(index).expect("the table is small");
        // Both operations that carry a `RuleAction`: the listener's default action and a
        // rule's action. The member the rule constrains is the same either way.
        let answers = [
            (
                format!("CreateListener defaultAction statusCode {status_code}"),
                create_listener(
                    &lattice,
                    &service_id,
                    serde_json::json!({
                        "name": format!("status-{status_code}"),
                        "protocol": "HTTP",
                        "port": port,
                        "defaultAction": { "fixedResponse": { "statusCode": status_code } },
                    }),
                )
                .await,
            ),
            (
                format!("CreateRule action statusCode {status_code}"),
                create_rule(
                    &lattice,
                    &service_id,
                    &listener_id,
                    rule_body(&format!("status-{status_code}"), priority, status_code),
                )
                .await,
            ),
        ];
        for (case, answer) in answers {
            assert_service_rule_refusal(&answer, &case, "statusCode");
            let message = message_of(&case, &answer);
            assert_eq!(
                message,
                "Provided status code is not supported.",
                "case {case:?}: the service rule's own sentence is the message, got {message:?}"
            );
        }
    }

    // The same rule ACCEPTS what the catalog holds, so the refusals above come from the
    // catalog and not from a check that refuses everything.
    for (index, status_code) in supported.into_iter().enumerate() {
        let port = 6_200 + i64::try_from(index).expect("the table is small");
        let accepted = create_listener(
            &lattice,
            &service_id,
            serde_json::json!({
                "name": format!("ok-{status_code}"),
                "protocol": "HTTP",
                "port": port,
                "defaultAction": { "fixedResponse": { "statusCode": status_code } },
            }),
        )
        .await;
        assert_eq!(
            accepted.status, 201,
            "case documented status code {status_code}: expected 201, got {} / {}",
            accepted.status, accepted.body
        );
    }

    // The other service rule of this path: `port` is modelled OPTIONAL and documents a
    // default for HTTP and HTTPS only, so under any other protocol the service's own rule
    // makes it mandatory — reported the same way, over the bare member `port`.
    let no_port = create_listener(
        &lattice,
        &service_id,
        listener_body("no-port", "TLS_PASSTHROUGH", None),
    )
    .await;
    assert_service_rule_refusal(&no_port, "TLS_PASSTHROUGH with no port", "port");

    // A MODELLED constraint is a different report: the framework's envelope, the member
    // PATH and the constraint clause. `ListenerName`'s `@pattern` is one.
    let bad_name = create_listener(
        &lattice,
        &service_id,
        listener_body("Upper-Case", "HTTP", Some(6_300)),
    )
    .await;
    assert_eq!(
        bad_name.status, 400,
        "case a name the modelled pattern forbids: expected 400, got {} / {}",
        bad_name.status, bad_name.body
    );
    let message = message_of("a name the modelled pattern forbids", &bad_name);
    assert!(
        message.starts_with("1 validation error detected: ")
            && message.contains("failed to satisfy constraint:"),
        "case a name the modelled pattern forbids: a modelled constraint keeps the \
         framework's construction, got {message:?}"
    );
}

/// A 400 of the shape a documented service RULE is reported with: one `fieldList` entry
/// naming the bare member the rule constrains, its message equal to the top-level
/// message, and no constraint-violation envelope.
fn assert_service_rule_refusal(answer: &Answer, case: &str, member: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected the ValidationException wire code, got {:?} / {}",
        answer.error_code,
        answer.body
    );
    assert_eq!(
        answer.body["reason"].as_str(),
        // The modelled ValidationExceptionReason for a member whose value was refused.
        Some("fieldValidationFailed"),
        "case {case:?}: wrong reason, got {}",
        answer.body
    );
    let fields = answer.body["fieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList, got {}", answer.body));
    assert_eq!(
        fields.len(),
        1,
        "case {case:?}: one member was refused, so fieldList holds one entry, got {}",
        answer.body
    );
    assert_eq!(
        fields[0]["name"].as_str(),
        Some(member),
        "case {case:?}: fieldList must name the bare member the rule constrains, got {}",
        answer.body
    );
    let message = message_of(case, answer);
    assert_eq!(
        fields[0]["message"].as_str(),
        Some(message.as_str()),
        "case {case:?}: the entry's message is the rule's sentence, the same one the \
         top-level message carries, got {}",
        answer.body
    );
    assert!(
        !message.contains("validation error detected")
            && !message.contains("failed to satisfy constraint"),
        "case {case:?}: a service rule is not a modelled constraint violation, so its \
         message carries no constraint envelope, got {message:?}"
    );
}

/// Rule 4: a duplicate-name conflict names the kind, the submitted name and the owning
/// resource the name is scoped to.
#[tokio::test]
async fn a_duplicate_name_conflict_names_the_kind_and_the_owning_resource() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "dup-name-host").await;

    // The rows differ in the name's length and shape: the shortest its `@length` allows,
    // a hyphenated one, one with digits, and the longest.
    let names: Vec<String> = vec![
        "abc".to_string(),
        "web-tier".to_string(),
        "api-2".to_string(),
        "l".repeat(63),
    ];

    for (index, name) in names.into_iter().enumerate() {
        let port = 5_000 + i64::try_from(index).expect("the table is small");
        let created = create_listener(
            &lattice,
            &service_id,
            listener_body(&name, "HTTP", Some(port)),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case listener {name:?}: the first create expected 201, got {} / {}",
            created.status, created.body
        );
        let listener_id = created.str_field("id");

        // The duplicate asks for a FREE port, so only the name can refuse it.
        let duplicate = create_listener(
            &lattice,
            &service_id,
            listener_body(&name, "HTTP", Some(port + 500)),
        )
        .await;
        let case = format!("duplicate listener name {name:?}");
        assert_conflict(&duplicate, &case, LISTENER_TYPE, &listener_id);
        let message = message_of(&case, &duplicate);
        assert_eq!(
            message,
            format!("Listener with name {name} already exists in {service_id} service."),
            "case {case:?}: the conflict must name the kind, the name and the owning \
             service, got {message:?}"
        );

        // The same rule over a name scoped to a LISTENER: a rule's name.
        let rule = create_rule(
            &lattice,
            &service_id,
            &listener_id,
            rule_body(&name, 10, 404),
        )
        .await;
        assert_eq!(
            rule.status, 201,
            "case rule {name:?}: the first create expected 201, got {} / {}",
            rule.status, rule.body
        );
        let rule_id = rule.str_field("id");
        let duplicate_rule = create_rule(
            &lattice,
            &service_id,
            &listener_id,
            // A free priority, so only the name can refuse it.
            rule_body(&name, 11, 404),
        )
        .await;
        let rule_case = format!("duplicate rule name {name:?}");
        assert_conflict(&duplicate_rule, &rule_case, RULE_TYPE, &rule_id);
        let rule_message = message_of(&rule_case, &duplicate_rule);
        assert_eq!(
            rule_message,
            format!("Rule with name {name} already exists in {listener_id} listener."),
            "case {rule_case:?}: the conflict must name the kind, the name and the owning \
             listener, got {rule_message:?}"
        );
    }
}
