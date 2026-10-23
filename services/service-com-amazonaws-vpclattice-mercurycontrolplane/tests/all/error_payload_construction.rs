// SPDX-License-Identifier: CC0-1.0

//! The STRUCTURED members of this service's error payloads: `resourceType`, the
//! not-found and conflict `message`, and the `ValidationException` construction.
//!
//! Every case drives the service the way a client does — a signed restJson1 request
//! through the router that mounts it — and every expectation is COMPUTED from the
//! request (the identifier sent, the name sent, the modelled pattern), so no row can
//! pass on a literal lifted from one recording.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the
//! rule generalises over (resource kind, error status, identifier form, the constraint
//! that was broken):
//!
//! 1. `resourceType` carries the resource-type TOKEN of the kind the error is about —
//!    the resource type in UPPER_SNAKE_CASE — on every 404 and every 409 path, and never
//!    the internal ARN path segment the same kind uses in an ARN or a name key.
//! 2. A not-found `message` is built from the request: the kind's name followed by the
//!    identifier exactly as the caller sent it (bare id or ARN) and "cannot be found.".
//! 3. A conflict `message` is built from the request identifiers: a duplicate name names
//!    the submitted name and the scope it must be unique in (the account, or the owning
//!    service for a listener); a duplicate association names both ARNs it would link.
//! 4. A `ValidationException` for a member whose value broke a constraint is AWS's
//!    validation-error construction: "<N> validation error(s) detected: " over the
//!    fragment "Value '<submitted value>' at '<member>' failed to satisfy constraint:
//!    <constraint>", with `fieldList` carrying that same fragment per member — and a
//!    value the same modelled pattern ALLOWS is accepted, so the check refuses on the
//!    pattern and not on everything.

use service_com_amazonaws_vpclattice_mercurycontrolplane::state;

use crate::vpclattice_service_slice::{as_label, expected_arn, message_of, Answer, Lattice};

/// `ServiceName`'s modelled `@pattern` as a validation error reports it: the pattern
/// without its anchors.
///
/// SOURCE: `smithy_query(query="shape",shape="ServiceName")` reports
/// `^(?!svc-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$`. Written out here rather than
/// imported from the crate, so the test pins the wire text against the MODEL and not
/// against whatever the code happens to hold.
const SERVICE_NAME_PATTERN: &str = r"(?!svc-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+";

/// `ServiceNetworkName`'s modelled `@pattern`, same source and same rendering.
const SERVICE_NETWORK_NAME_PATTERN: &str = r"(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+";

/// `ListenerName`'s modelled `@pattern`, same source and same rendering.
const LISTENER_NAME_PATTERN: &str = r"(?!listener-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+";

/// An id of `prefix` that no create of this service can ever have minted: the 17
/// characters are all `0`.
fn absent_id(prefix: &str) -> String {
    format!("{prefix}{}", "0".repeat(17))
}

/// The fragment AWS reports for one refused member value.
fn fragment(member: &str, value: &str, constraint: &str) -> String {
    format!("Value '{value}' at '{member}' failed to satisfy constraint: {constraint}")
}

/// AWS's constraint wording for a modelled `@pattern`.
fn pattern_constraint(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// `POST /services/{serviceIdentifier}/listeners` with a minimal HTTP listener body.
async fn create_http_listener(lattice: &Lattice, service_identifier: &str, name: &str) -> Answer {
    lattice
        .call(
            "POST",
            &format!("/services/{service_identifier}/listeners"),
            Some(serde_json::json!({
                "name": name,
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            })),
        )
        .await
}

/// `POST /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules` with a
/// minimal fixed-response rule body.
async fn create_fixed_response_rule(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
    name: &str,
    priority: i64,
) -> Answer {
    lattice
        .call(
            "POST",
            &format!("/services/{service_identifier}/listeners/{listener_identifier}/rules"),
            Some(serde_json::json!({
                "name": name,
                "priority": priority,
                "match": { "httpMatch": { "pathMatch": {
                    "match": { "exact": "/token" },
                    "caseSensitive": true,
                } } },
                "action": { "fixedResponse": { "statusCode": 404 } },
            })),
        )
        .await
}

/// One service network, one service and the association between them.
struct Linked {
    network_id: String,
    network_arn: String,
    service_id: String,
    service_arn: String,
    association_id: String,
}

/// Create a service network, a service and the association of the two.
async fn linked(lattice: &Lattice, row: &str) -> Linked {
    let network = lattice
        .create_service_network(serde_json::json!({ "name": format!("net-{row}") }))
        .await;
    assert_eq!(
        network.status, 201,
        "case {row:?}: creating the service network expected 201, got {} / {}",
        network.status, network.body
    );
    let service = lattice
        .create_service(serde_json::json!({ "name": format!("app-{row}") }))
        .await;
    assert_eq!(
        service.status, 201,
        "case {row:?}: creating the service expected 201, got {} / {}",
        service.status, service.body
    );
    let network_id = network.str_field("id");
    let service_id = service.str_field("id");
    let association = lattice.associate(&service_id, &network_id).await;
    assert_eq!(
        association.status, 200,
        "case {row:?}: associating expected 200, got {} / {}",
        association.status, association.body
    );
    Linked {
        network_id,
        network_arn: network.str_field("arn"),
        service_id,
        service_arn: service.str_field("arn"),
        association_id: association.str_field("id"),
    }
}

/// Rule 1: every 404 and 409 of every resource kind reports the kind's wire token.
#[tokio::test]
async fn error_resource_type_is_the_aws_token_for_every_kind_and_status() {
    let lattice = Lattice::new();
    let fixture = linked(&lattice, "token").await;
    let host = lattice
        .create_service(serde_json::json!({ "name": "listener-token-host" }))
        .await;
    assert_eq!(
        host.status, 201,
        "case fixture listener host: expected 201, got {} / {}",
        host.status, host.body
    );
    let listener_host_id = host.str_field("id");
    let listener = create_http_listener(&lattice, &listener_host_id, "taken-listener").await;
    assert_eq!(
        listener.status, 201,
        "case fixture listener: expected 201, got {} / {}",
        listener.status, listener.body
    );
    let listener_id = listener.str_field("id");
    // A rule of that listener, so the rule kind's 404 and 409 paths are both reachable.
    let rule = create_fixed_response_rule(
        &lattice,
        &listener_host_id,
        &listener_id,
        "taken-rule",
        10,
    )
    .await;
    assert_eq!(
        rule.status, 201,
        "case fixture rule: expected 201, got {} / {}",
        rule.status, rule.body
    );

    // (case, the answer, the expected status, the expected token, the ARN path segment
    // the same kind uses — which the token must NOT be)
    let cases: Vec<(&str, Answer, u16, &str, &str)> = vec![
        (
            "404 GetServiceNetwork",
            lattice
                .get_service_network(&absent_id(state::PREFIX_SERVICE_NETWORK))
                .await,
            404,
            "SERVICE_NETWORK",
            state::KIND_SERVICE_NETWORK,
        ),
        (
            "404 DeleteServiceNetwork",
            lattice
                .call(
                    "DELETE",
                    &format!(
                        "/servicenetworks/{}",
                        absent_id(state::PREFIX_SERVICE_NETWORK)
                    ),
                    None,
                )
                .await,
            404,
            "SERVICE_NETWORK",
            state::KIND_SERVICE_NETWORK,
        ),
        (
            "404 UpdateServiceNetwork",
            lattice
                .call(
                    "PATCH",
                    &format!(
                        "/servicenetworks/{}",
                        absent_id(state::PREFIX_SERVICE_NETWORK)
                    ),
                    Some(serde_json::json!({ "authType": "NONE" })),
                )
                .await,
            404,
            "SERVICE_NETWORK",
            state::KIND_SERVICE_NETWORK,
        ),
        (
            "404 GetService",
            lattice.get_service(&absent_id(state::PREFIX_SERVICE)).await,
            404,
            "SERVICE",
            state::KIND_SERVICE,
        ),
        (
            "404 DeleteService",
            lattice
                .call(
                    "DELETE",
                    &format!("/services/{}", absent_id(state::PREFIX_SERVICE)),
                    None,
                )
                .await,
            404,
            "SERVICE",
            state::KIND_SERVICE,
        ),
        (
            "404 GetServiceNetworkServiceAssociation",
            lattice
                .call(
                    "GET",
                    &format!(
                        "/servicenetworkserviceassociations/{}",
                        absent_id(state::PREFIX_SERVICE_ASSOCIATION)
                    ),
                    None,
                )
                .await,
            404,
            "SERVICE_NETWORK_SERVICE_ASSOCIATION",
            state::KIND_SERVICE_ASSOCIATION,
        ),
        (
            "404 DeleteServiceNetworkServiceAssociation",
            lattice
                .call(
                    "DELETE",
                    &format!(
                        "/servicenetworkserviceassociations/{}",
                        absent_id(state::PREFIX_SERVICE_ASSOCIATION)
                    ),
                    None,
                )
                .await,
            404,
            "SERVICE_NETWORK_SERVICE_ASSOCIATION",
            state::KIND_SERVICE_ASSOCIATION,
        ),
        (
            "404 GetListener",
            lattice
                .call(
                    "GET",
                    &format!(
                        "/services/{listener_host_id}/listeners/{}",
                        absent_id(state::PREFIX_LISTENER)
                    ),
                    None,
                )
                .await,
            404,
            "LISTENER",
            state::KIND_LISTENER,
        ),
        (
            "404 GetRule",
            lattice
                .call(
                    "GET",
                    &format!(
                        "/services/{listener_host_id}/listeners/{listener_id}/rules/{}",
                        absent_id(state::PREFIX_RULE)
                    ),
                    None,
                )
                .await,
            404,
            "RULE",
            state::KIND_RULE,
        ),
        (
            "409 CreateServiceNetwork on a taken name",
            lattice
                .create_service_network(serde_json::json!({ "name": "net-token" }))
                .await,
            409,
            "SERVICE_NETWORK",
            state::KIND_SERVICE_NETWORK,
        ),
        (
            "409 CreateService on a taken name",
            lattice
                .create_service(serde_json::json!({ "name": "app-token" }))
                .await,
            409,
            "SERVICE",
            state::KIND_SERVICE,
        ),
        (
            "409 CreateServiceNetworkServiceAssociation on an existing pair",
            lattice
                .associate(&fixture.service_id, &fixture.network_id)
                .await,
            409,
            "SERVICE_NETWORK_SERVICE_ASSOCIATION",
            state::KIND_SERVICE_ASSOCIATION,
        ),
        (
            "409 DeleteServiceNetwork while a service is associated",
            lattice
                .call(
                    "DELETE",
                    &format!("/servicenetworks/{}", fixture.network_id),
                    None,
                )
                .await,
            409,
            "SERVICE_NETWORK",
            state::KIND_SERVICE_NETWORK,
        ),
        (
            "409 DeleteService while it is associated",
            lattice
                .call("DELETE", &format!("/services/{}", fixture.service_id), None)
                .await,
            409,
            "SERVICE",
            state::KIND_SERVICE,
        ),
        (
            "409 CreateListener on a taken name",
            create_http_listener(&lattice, &listener_host_id, "taken-listener").await,
            409,
            "LISTENER",
            state::KIND_LISTENER,
        ),
        (
            // "Each rule for a specific listener must have a unique priority", so a
            // second rule asking for a taken priority is the rule kind's conflict.
            "409 CreateRule on a taken priority",
            create_fixed_response_rule(
                &lattice,
                &listener_host_id,
                &listener_id,
                "second-rule",
                10,
            )
            .await,
            409,
            "RULE",
            state::KIND_RULE,
        ),
    ];

    for (case, answer, expected_status, expected_token, internal_key) in cases {
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status}, got {} / {}",
            answer.status, answer.body
        );
        let reported = answer.body["resourceType"].as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the exception must carry the required resourceType member, got {}",
                answer.body
            )
        });
        assert_eq!(
            reported, expected_token,
            "case {case:?}: expected resourceType {expected_token:?}, got {reported:?}"
        );
        assert_ne!(
            reported, internal_key,
            "case {case:?}: resourceType must be the AWS token, not the internal ARN key"
        );
        assert!(
            reported
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'_'),
            "case {case:?}: a resource-type token is UPPER_SNAKE_CASE, got {reported:?}"
        );
        // `resourceId` names the resource the error is about, so the pair identifies it.
        assert!(
            answer.body["resourceId"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: the exception must carry a non-empty resourceId, got {}",
            answer.body
        );
    }

    // The association is still there: the two refused deletes above changed nothing.
    let association = lattice
        .call(
            "GET",
            &format!(
                "/servicenetworkserviceassociations/{}",
                fixture.association_id
            ),
            None,
        )
        .await;
    assert_eq!(
        association.status, 200,
        "case refused deletes: the association must survive, got {} / {}",
        association.status, association.body
    );
}

/// Rule 2: the not-found message names the kind and the identifier the caller sent.
#[tokio::test]
async fn not_found_messages_name_the_kind_and_the_identifier_as_sent() {
    let lattice = Lattice::new();

    // (case, request path template, the kind name the message must use, the ARN kind the
    // identifier's ARN form is built from, the id prefix)
    let kinds: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "service network",
            "/servicenetworks/{}",
            "ServiceNetwork",
            state::KIND_SERVICE_NETWORK,
            state::PREFIX_SERVICE_NETWORK,
        ),
        (
            "service",
            "/services/{}",
            "Service",
            state::KIND_SERVICE,
            state::PREFIX_SERVICE,
        ),
        (
            "association",
            "/servicenetworkserviceassociations/{}",
            "Association",
            state::KIND_SERVICE_ASSOCIATION,
            state::PREFIX_SERVICE_ASSOCIATION,
        ),
    ];

    for (kind, path_template, message_name, arn_kind, prefix) in kinds {
        let id = absent_id(prefix);
        let arn = expected_arn(arn_kind, &id);
        // Both modelled identifier forms, and both the reading and the deleting path.
        for (form, identifier) in [("bare id", id.clone()), ("ARN", arn.clone())] {
            for method in ["GET", "DELETE"] {
                let case = format!("{kind} by {form} via {method}");
                let answer = lattice
                    .call(
                        method,
                        &path_template.replace("{}", &as_label(&identifier)),
                        None,
                    )
                    .await;
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}: expected 404, got {} / {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case:?}: expected ResourceNotFoundException, got {:?}",
                    answer.error_code
                );
                let message = message_of(&case, &answer);
                assert_eq!(
                    message,
                    format!("{message_name} {identifier} cannot be found."),
                    "case {case:?}: the message must name the kind and the identifier sent, \
                     got {message:?}"
                );
                assert_eq!(
                    answer.body["resourceId"].as_str(),
                    Some(identifier.as_str()),
                    "case {case:?}: resourceId must be the identifier sent, got {}",
                    answer.body
                );
            }
        }
    }

    // The update path takes the same identifier as the reads and must report the same
    // construction.
    let absent_network = absent_id(state::PREFIX_SERVICE_NETWORK);
    let updated = lattice
        .call(
            "PATCH",
            &format!("/servicenetworks/{absent_network}"),
            Some(serde_json::json!({ "authType": "AWS_IAM" })),
        )
        .await;
    assert_eq!(
        updated.status, 404,
        "case service network by bare id via PATCH: expected 404, got {} / {}",
        updated.status, updated.body
    );
    let message = message_of("service network by bare id via PATCH", &updated);
    assert_eq!(
        message,
        format!("ServiceNetwork {absent_network} cannot be found."),
        "case service network by bare id via PATCH: wrong message, got {message:?}"
    );

    // A listener, whose identifier is scoped by its service: the same construction.
    let service = lattice
        .create_service(serde_json::json!({ "name": "notfound-host" }))
        .await;
    let service_id = service.str_field("id");
    let listener_id = absent_id(state::PREFIX_LISTENER);
    let answer = lattice
        .call(
            "GET",
            &format!("/services/{service_id}/listeners/{listener_id}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 404,
        "case listener by bare id: expected 404, got {} / {}",
        answer.status, answer.body
    );
    let message = message_of("listener by bare id", &answer);
    assert_eq!(
        message,
        format!("Listener {listener_id} cannot be found."),
        "case listener by bare id: wrong message, got {message:?}"
    );
}

/// Rule 3: a conflict message is built from the identifiers the request carried.
#[tokio::test]
async fn conflict_messages_are_built_from_the_request_identifiers() {
    let lattice = Lattice::new();

    // A duplicate name, over rows that vary the name's length and shape and the resource
    // kind whose name it is.
    let duplicate_names: Vec<(&str, &str, String)> = vec![
        ("shortest legal name", "/servicenetworks", "net".to_string()),
        (
            "hyphenated name",
            "/servicenetworks",
            "dup-net-one".to_string(),
        ),
        (
            "longest legal service-network name",
            "/servicenetworks",
            "n".repeat(63),
        ),
        ("service, short name", "/services", "app".to_string()),
        (
            "service, digits and hyphens",
            "/services",
            "app-2-dup".to_string(),
        ),
        ("longest legal service name", "/services", "s".repeat(40)),
    ];

    for (case, path, name) in duplicate_names {
        let created = lattice
            .call("POST", path, Some(serde_json::json!({ "name": name })))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the first create expected 201, got {} / {}",
            created.status, created.body
        );
        let again = lattice
            .call("POST", path, Some(serde_json::json!({ "name": name })))
            .await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: the second create expected 409, got {} / {}",
            again.status, again.body
        );
        let message = message_of(case, &again);
        assert_eq!(
            message,
            format!(
                "Resource with name {name} already exists in account {}.",
                state::account_id()
            ),
            "case {case:?}: the conflict must name the submitted name and the account, \
             got {message:?}"
        );
        assert_eq!(
            again.body["resourceId"].as_str(),
            created.body["id"].as_str(),
            "case {case:?}: resourceId must name the resource that holds the name, got {}",
            again.body
        );
    }

    // A duplicate association names both ARNs it would link, whichever identifier form
    // the second request used.
    for (row, by_arn) in [("bare-ids", false), ("arns", true)] {
        let fixture = linked(&lattice, row).await;
        let (service_identifier, network_identifier) = if by_arn {
            (fixture.service_arn.clone(), fixture.network_arn.clone())
        } else {
            (fixture.service_id.clone(), fixture.network_id.clone())
        };
        let again = lattice
            .associate(&service_identifier, &network_identifier)
            .await;
        assert_eq!(
            again.status, 409,
            "case association by {row}: expected 409, got {} / {}",
            again.status, again.body
        );
        let case = format!("association by {row}");
        let message = message_of(&case, &again);
        assert_eq!(
            message,
            format!(
                "Association for {} and {} already exists.",
                fixture.network_arn, fixture.service_arn
            ),
            "case {case:?}: the conflict must name both ARNs it links, got {message:?}"
        );
        assert_eq!(
            again.body["resourceId"].as_str(),
            Some(fixture.association_id.as_str()),
            "case {case:?}: resourceId must name the existing association, got {}",
            again.body
        );
    }

    // A listener name is unique within its SERVICE, so its conflict names that scope
    // rather than the account.
    let host = lattice
        .create_service(serde_json::json!({ "name": "listener-conflict-host" }))
        .await;
    let host_id = host.str_field("id");
    let first = create_http_listener(&lattice, &host_id, "only-listener").await;
    assert_eq!(
        first.status, 201,
        "case listener first create: expected 201, got {} / {}",
        first.status, first.body
    );
    let again = create_http_listener(&lattice, &host_id, "only-listener").await;
    assert_eq!(
        again.status, 409,
        "case listener duplicate name: expected 409, got {} / {}",
        again.status, again.body
    );
    let message = message_of("listener duplicate name", &again);
    assert_eq!(
        message,
        // A name scoped to an owning RESOURCE names the kind, the submitted name and
        // that resource — not the account form the two rows above assert.
        format!("Listener with name only-listener already exists in {host_id} service."),
        "case listener duplicate name: the conflict must name the name and the owning \
         service, got {message:?}"
    );
}

/// Rule 4: a refused member value is reported in AWS's validation-error construction.
#[tokio::test]
async fn validation_errors_use_the_aws_validation_error_construction() {
    let lattice = Lattice::new();
    let host = lattice
        .create_service(serde_json::json!({ "name": "validation-host" }))
        .await;
    let host_id = host.str_field("id");

    // Every way the three name patterns can be broken — the reserved id prefix, an
    // uppercase character, a leading hyphen, a trailing hyphen and two adjacent hyphens —
    // across all three constrained name members. The two cases the issue cited are two
    // rows of this table.
    // (case, path, request body, member, submitted value, modelled pattern)
    let mut rows: Vec<(String, String, serde_json::Value, &str, String, &str)> = Vec::new();
    for (bad, label) in [
        ("svc-reserved-prefix", "reserved id prefix"),
        ("Upper-Case", "uppercase character"),
        ("-leading-hyphen", "leading hyphen"),
        ("trailing-hyphen-", "trailing hyphen"),
        ("two--hyphens", "two adjacent hyphens"),
        ("under_score", "character outside the class"),
    ] {
        rows.push((
            format!("service name with a {label}"),
            "/services".to_string(),
            serde_json::json!({ "name": bad }),
            "name",
            bad.to_string(),
            SERVICE_NAME_PATTERN,
        ));
    }
    for (bad, label) in [
        ("Upper-Case", "uppercase character"),
        ("-leading-hyphen", "leading hyphen"),
        ("trailing-hyphen-", "trailing hyphen"),
        ("two--hyphens", "two adjacent hyphens"),
        ("dot.separated", "character outside the class"),
    ] {
        rows.push((
            format!("service-network name with a {label}"),
            "/servicenetworks".to_string(),
            serde_json::json!({ "name": bad }),
            "name",
            bad.to_string(),
            SERVICE_NETWORK_NAME_PATTERN,
        ));
    }
    for (bad, label) in [
        ("listener-reserved", "reserved id prefix"),
        ("Upper-Case", "uppercase character"),
        ("two--hyphens", "two adjacent hyphens"),
    ] {
        rows.push((
            format!("listener name with a {label}"),
            format!("/services/{host_id}/listeners"),
            serde_json::json!({
                "name": bad,
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            }),
            "name",
            bad.to_string(),
            LISTENER_NAME_PATTERN,
        ));
    }

    for (case, path, body, member, value, pattern) in rows {
        let answer = lattice.call("POST", &path, Some(body)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code
        );
        let expected_fragment = fragment(member, &value, &pattern_constraint(pattern));
        let message = message_of(&case, &answer);
        assert_eq!(
            message,
            format!("1 validation error detected: {expected_fragment}"),
            "case {case:?}: wrong validation message, got {message:?}"
        );
        assert_eq!(
            answer.body["reason"].as_str(),
            // The modelled ValidationExceptionReason for a member that failed validation.
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
            "case {case:?}: fieldList must name the refused member, got {}",
            answer.body
        );
        assert_eq!(
            fields[0]["message"].as_str(),
            Some(expected_fragment.as_str()),
            "case {case:?}: fieldList carries the fragment without the count, got {}",
            answer.body
        );
    }

    // A map whose size broke its `@length` ceiling: the same construction, with the
    // value left unquoted because it is not a scalar the message can render.
    let too_many_tags = lattice
        .create_service(serde_json::json!({
            "name": "tag-ceiling",
            "tags": (0..=state::MAX_TAGS)
                .map(|index| (format!("key-{index}"), serde_json::json!("v")))
                .collect::<serde_json::Map<String, serde_json::Value>>(),
        }))
        .await;
    assert_eq!(
        too_many_tags.status, 400,
        "case tags past the ceiling: expected 400, got {} / {}",
        too_many_tags.status, too_many_tags.body
    );
    let expected = format!(
        "Value '{} entries' at 'tags' failed to satisfy constraint: Member must have length \
         less than or equal to {}",
        state::MAX_TAGS + 1,
        state::MAX_TAGS
    );
    let message = message_of("tags past the ceiling", &too_many_tags);
    assert_eq!(
        message,
        format!("1 validation error detected: {expected}"),
        "case tags past the ceiling: wrong validation message, got {message:?}"
    );

    // The same patterns ACCEPT what they allow, so the refusals above come from the
    // pattern and not from a check that refuses everything: each row is a value at or
    // just inside a published bound of its pattern.
    let accepted: Vec<(&str, String, serde_json::Value)> = vec![
        (
            "service name that merely starts with svc (no hyphen)",
            "/services".to_string(),
            serde_json::json!({ "name": "svcorders" }),
        ),
        (
            "shortest legal service name",
            "/services".to_string(),
            serde_json::json!({ "name": "a-b" }),
        ),
        (
            "longest legal service name",
            "/services".to_string(),
            serde_json::json!({ "name": "s".repeat(40) }),
        ),
        (
            "service-network name of digits and hyphens",
            "/servicenetworks".to_string(),
            serde_json::json!({ "name": "9-net-0" }),
        ),
        (
            "longest legal service-network name",
            "/servicenetworks".to_string(),
            serde_json::json!({ "name": "n".repeat(63) }),
        ),
        (
            "listener name that merely starts with listener (no hyphen)",
            format!("/services/{host_id}/listeners"),
            serde_json::json!({
                "name": "listenerish",
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            }),
        ),
    ];

    for (case, path, body) in accepted {
        let answer = lattice.call("POST", &path, Some(body)).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: the pattern allows this value, so expected 201, got {} / {}",
            answer.status, answer.body
        );
    }
}
