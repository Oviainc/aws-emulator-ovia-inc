// SPDX-License-Identifier: CC0-1.0

//! The refusal MESSAGE of the two referential-integrity delete guards.
//!
//! The rule under test: when a delete is refused because something is still associated
//! with the resource, the `ConflictException` the service answers with carries the
//! operation's fixed message template over the identifier the request RESOLVED to (the
//! bare id, whichever of the modelled identifier forms the caller sent), and reports no
//! count of the dependents it found — the number of associations decides the refusal but
//! is not part of what the service reports.
//!
//! Each case is asserted over a TABLE whose rows vary what the rule generalises over:
//! the number of blocking associations (so a count cannot leak into the message), the
//! identifier form in the request path (bare id and ARN, so the message carries the
//! resolved id either way), and the resource itself (a fresh id per row, so no row can
//! pass on a literal). Alongside the message, each row re-checks what already held: the
//! HTTP status, the error code, the exception's `resourceId`/`resourceType`, and that the
//! refusal left the resource and its associations intact.

use crate::vpclattice_service_slice::{as_label, message_of, Lattice};

/// How many blocking associations each row sets up before the delete. More than one row
/// has more than one association, which is what a message carrying a count would reveal.
const BLOCKING_ASSOCIATION_COUNTS: [usize; 3] = [1, 2, 3];

/// AWS's refusal message for `DeleteServiceNetwork` while a service is associated with
/// the service network.
///
/// Source: recorded AWS behaviour — the Smithy model gives `ConflictException` a required
/// `message` but states no text for this guard. Two independent recordings of the refusal
/// (different accounts, different service-network ids) carry this identical wording with
/// only the resolved id substituted, which is what makes it a template rather than one
/// recorded string.
fn expected_service_network_refusal(service_network_id: &str) -> String {
    format!("ServiceNetwork {service_network_id} has service(s) associated")
}

/// AWS's refusal message for `DeleteService` while the service is associated with a
/// service network. Same source and same form; the trailing period is part of it.
fn expected_service_refusal(service_id: &str) -> String {
    format!("Service {service_id} has one or more associated service networks.")
}

/// Whether `message` reports a number other than the one inside the resolved id: a count
/// of dependents leaking into the refusal reason.
fn reports_a_count(message: &str, resolved_id: &str) -> bool {
    message
        .replace(resolved_id, "")
        .chars()
        .any(|c| c.is_ascii_digit())
}

/// One service network with `count` services associated with it: the ids of the network,
/// its ARN, the services, and the associations that block the deletes.
struct Linked {
    network_id: String,
    network_arn: String,
    service_ids: Vec<String>,
    service_arns: Vec<String>,
    association_ids: Vec<String>,
}

/// Create one service network and `count` services, each associated with it.
async fn link_services(lattice: &Lattice, row: &str, count: usize) -> Linked {
    let network = lattice
        .create_service_network(serde_json::json!({ "name": format!("net-{row}") }))
        .await;
    assert_eq!(
        network.status, 201,
        "case {row:?}: creating the service network expected 201, got {} / {}",
        network.status, network.body
    );
    let mut linked = Linked {
        network_id: network.str_field("id"),
        network_arn: network.str_field("arn"),
        service_ids: Vec::new(),
        service_arns: Vec::new(),
        association_ids: Vec::new(),
    };
    for nth in 0..count {
        let service = lattice
            .create_service(serde_json::json!({ "name": format!("app-{row}-{nth}") }))
            .await;
        assert_eq!(
            service.status, 201,
            "case {row:?} service {nth}: creating the service expected 201, got {} / {}",
            service.status, service.body
        );
        let service_id = service.str_field("id");
        let association = lattice.associate(&service_id, &linked.network_id).await;
        assert_eq!(
            association.status, 200,
            "case {row:?} service {nth}: associating expected 200, got {} / {}",
            association.status, association.body
        );
        linked.association_ids.push(association.str_field("id"));
        linked.service_arns.push(service.str_field("arn"));
        linked.service_ids.push(service_id);
    }
    linked
}

/// Create `count` service networks and associate one service with every one of them.
async fn link_networks(
    lattice: &Lattice,
    row: &str,
    count: usize,
) -> (String, String, Vec<String>) {
    let service = lattice
        .create_service(serde_json::json!({ "name": format!("app-{row}") }))
        .await;
    assert_eq!(
        service.status, 201,
        "case {row:?}: creating the service expected 201, got {} / {}",
        service.status, service.body
    );
    let service_id = service.str_field("id");
    let mut association_ids = Vec::new();
    for nth in 0..count {
        let network = lattice
            .create_service_network(serde_json::json!({ "name": format!("net-{row}-{nth}") }))
            .await;
        assert_eq!(
            network.status, 201,
            "case {row:?} network {nth}: creating the service network expected 201, got {} / {}",
            network.status, network.body
        );
        let association = lattice
            .associate(&service_id, &network.str_field("id"))
            .await;
        assert_eq!(
            association.status, 200,
            "case {row:?} network {nth}: associating expected 200, got {} / {}",
            association.status, association.body
        );
        association_ids.push(association.str_field("id"));
    }
    (service_id, service.str_field("arn"), association_ids)
}

/// `DeleteServiceNetwork`'s guard: the AWS template over the resolved id, no count, and
/// nothing changed by the refusal.
#[tokio::test]
async fn refusing_to_delete_an_associated_service_network_reports_the_aws_template() {
    let lattice = Lattice::new();

    for count in BLOCKING_ASSOCIATION_COUNTS {
        for by_arn in [false, true] {
            let row = format!("{count}-assoc-{}", if by_arn { "arn" } else { "id" });
            let linked = link_services(&lattice, &row, count).await;
            let identifier = if by_arn {
                as_label(&linked.network_arn)
            } else {
                linked.network_id.clone()
            };

            let refused = lattice
                .call("DELETE", &format!("/servicenetworks/{identifier}"), None)
                .await;
            assert_eq!(
                refused.status, 409,
                "case {row:?}: expected 409, got {} / {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("ConflictException"),
                "case {row:?}: expected the ConflictException error code, got {:?}",
                refused.error_code
            );

            let message = message_of(&row, &refused);
            assert_eq!(
                message,
                expected_service_network_refusal(&linked.network_id),
                "case {row:?}: wrong refusal message, got {message:?}"
            );
            assert!(
                !reports_a_count(&message, &linked.network_id),
                "case {row:?}: the refusal must not report how many associations it found, \
                 got {message:?}"
            );
            assert_eq!(
                refused.body["resourceId"].as_str(),
                Some(linked.network_id.as_str()),
                "case {row:?}: resourceId must name the resolved service network, got {}",
                refused.body
            );
            assert_eq!(
                refused.body["resourceType"].as_str(),
                // The resource-type token AWS reports for a service network; see the
                // error_payload_construction suite for the rule it follows.
                Some("SERVICE_NETWORK"),
                "case {row:?}: wrong resourceType, got {}",
                refused.body
            );

            // The refusal changed nothing: the service network and every association it
            // blocked on are still there.
            let still_there = lattice.get_service_network(&linked.network_id).await;
            assert_eq!(
                still_there.status, 200,
                "case {row:?}: the refused delete must leave the service network, got {} / {}",
                still_there.status, still_there.body
            );
            assert_eq!(
                still_there.body["numberOfAssociatedServices"].as_i64(),
                Some(count as i64),
                "case {row:?}: expected {count} associated services after the refusal, got {}",
                still_there.body
            );

            // With the associations gone the same delete succeeds, so the guard refused
            // on the associations and not on the resource.
            for association_id in &linked.association_ids {
                let removed = lattice
                    .call(
                        "DELETE",
                        &format!("/servicenetworkserviceassociations/{association_id}"),
                        None,
                    )
                    .await;
                assert_eq!(
                    removed.status, 200,
                    "case {row:?}: deleting association {association_id} expected 200, got {} / {}",
                    removed.status, removed.body
                );
            }
            let deleted = lattice
                .call("DELETE", &format!("/servicenetworks/{identifier}"), None)
                .await;
            assert_eq!(
                deleted.status, 204,
                "case {row:?}: once unassociated the delete expected 204, got {} / {}",
                deleted.status, deleted.body
            );

            for service_id in &linked.service_ids {
                let gone = lattice
                    .call("DELETE", &format!("/services/{service_id}"), None)
                    .await;
                assert_eq!(
                    gone.status, 200,
                    "case {row:?}: cleaning up service {service_id} expected 200, got {} / {}",
                    gone.status, gone.body
                );
            }
        }
    }
}

/// `DeleteService`'s guard: the same rule, with that operation's own template.
#[tokio::test]
async fn refusing_to_delete_an_associated_service_reports_the_aws_template() {
    let lattice = Lattice::new();

    for count in BLOCKING_ASSOCIATION_COUNTS {
        for by_arn in [false, true] {
            let row = format!("{count}-net-{}", if by_arn { "arn" } else { "id" });
            let (service_id, service_arn, association_ids) =
                link_networks(&lattice, &row, count).await;
            let identifier = if by_arn {
                as_label(&service_arn)
            } else {
                service_id.clone()
            };

            let refused = lattice
                .call("DELETE", &format!("/services/{identifier}"), None)
                .await;
            assert_eq!(
                refused.status, 409,
                "case {row:?}: expected 409, got {} / {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("ConflictException"),
                "case {row:?}: expected the ConflictException error code, got {:?}",
                refused.error_code
            );

            let message = message_of(&row, &refused);
            assert_eq!(
                message,
                expected_service_refusal(&service_id),
                "case {row:?}: wrong refusal message, got {message:?}"
            );
            assert!(
                !reports_a_count(&message, &service_id),
                "case {row:?}: the refusal must not report how many associations it found, \
                 got {message:?}"
            );
            assert_eq!(
                refused.body["resourceId"].as_str(),
                Some(service_id.as_str()),
                "case {row:?}: resourceId must name the resolved service, got {}",
                refused.body
            );
            assert_eq!(
                refused.body["resourceType"].as_str(),
                // The resource-type token AWS reports for a service.
                Some("SERVICE"),
                "case {row:?}: wrong resourceType, got {}",
                refused.body
            );

            let still_there = lattice.get_service(&service_id).await;
            assert_eq!(
                still_there.status, 200,
                "case {row:?}: the refused delete must leave the service, got {} / {}",
                still_there.status, still_there.body
            );

            for association_id in &association_ids {
                let removed = lattice
                    .call(
                        "DELETE",
                        &format!("/servicenetworkserviceassociations/{association_id}"),
                        None,
                    )
                    .await;
                assert_eq!(
                    removed.status, 200,
                    "case {row:?}: deleting association {association_id} expected 200, got {} / {}",
                    removed.status, removed.body
                );
            }
            let deleted = lattice
                .call("DELETE", &format!("/services/{identifier}"), None)
                .await;
            assert_eq!(
                deleted.status, 200,
                "case {row:?}: once unassociated the delete expected 200, got {} / {}",
                deleted.status, deleted.body
            );
        }
    }
}
