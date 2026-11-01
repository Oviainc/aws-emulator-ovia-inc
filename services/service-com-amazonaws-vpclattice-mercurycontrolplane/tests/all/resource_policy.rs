// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource-policy slice: `PutResourcePolicy`,
//! `GetResourcePolicy` and `DeleteResourcePolicy`, each driven through the router the way
//! a client drives it and each read back out of a LATER request, so nothing here can pass
//! on a canned body.
//!
//! The harness (`Lattice`, `Answer`, `as_label`, …) is the one the service-slice suite
//! beside this file already builds; nothing is duplicated here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. A resource-based policy belongs to the RESOURCE its `resourceArn` names: the
//!    document a put submits is the document a later get reports, verbatim; a second put
//!    replaces it; a change to anything else — the resource's own members, another
//!    resource's policy — leaves it exactly as it was; and the delete removes it, after
//!    which the get reports no policy at all.
//! 2. The policy operations resolve the resource their ARN names before touching a
//!    policy: an ARN of the modelled shape naming no persisted resource is a 404
//!    `ResourceNotFoundException` built over the kind the ARN named, and an input outside
//!    a modelled constraint is a 400 — in neither case is anything stored.
//!
//! The table spans what rule 1 generalises over: the KIND the policy is attached to
//! (service network and service, the kinds this emulator can hold), and the FORM of the
//! document (statement count, padding whitespace, non-ASCII characters, and a document
//! close to the `PolicyString` ceiling). The recorded case — one document on one service
//! network, surviving an unrelated update and then cleared — is one row of it.

use crate::resource_not_found_messages::absent_id;
use crate::vpclattice_service_slice::{as_label, expected_arn, Answer, Lattice};

/// `PUT /resourcepolicy/{resourceArn}`.
async fn put_policy(lattice: &Lattice, resource_arn: &str, policy: &str) -> Answer {
    lattice
        .call(
            "PUT",
            &format!("/resourcepolicy/{}", as_label(resource_arn)),
            Some(serde_json::json!({ "policy": policy })),
        )
        .await
}

/// `GET /resourcepolicy/{resourceArn}`.
async fn get_policy(lattice: &Lattice, resource_arn: &str) -> Answer {
    lattice
        .call(
            "GET",
            &format!("/resourcepolicy/{}", as_label(resource_arn)),
            None,
        )
        .await
}

/// `DELETE /resourcepolicy/{resourceArn}`.
async fn delete_policy(lattice: &Lattice, resource_arn: &str) -> Answer {
    lattice
        .call(
            "DELETE",
            &format!("/resourcepolicy/{}", as_label(resource_arn)),
            None,
        )
        .await
}

/// The ARN resource segments of the kinds a `resourceArn` can name, from that member's
/// own modelled pattern (`servicenetwork/sn-`, `service/svc-`,
/// `resourceconfiguration/rcfg-`; `smithy_query(query="shape",shape="ResourceArn")`).
const ARN_KIND_SERVICE_NETWORK: &str = "servicenetwork";
const ARN_KIND_SERVICE: &str = "service";
const ARN_KIND_RESOURCE_CONFIGURATION: &str = "resourceconfiguration";

/// The id prefixes of those same kinds, from the same pattern.
const PREFIX_SERVICE_NETWORK: &str = "sn-";
const PREFIX_SERVICE: &str = "svc-";
const PREFIX_RESOURCE_CONFIGURATION: &str = "rcfg-";

/// The `@length` ceiling the model puts on `PolicyString` (`len 1..10000`).
const POLICY_MAX_LENGTH: usize = 10_000;

/// The policy documents the table spans. No single document is the requirement: the rule
/// is that whatever document the caller submits is the one a later read reports, so the
/// rows differ in statement count, in padding whitespace, in the characters they carry
/// and in length.
fn policy_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "one statement",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"share","Effect":"Allow","Principal":{"AWS":"123456789012"},"Action":"vpc-lattice:CreateServiceNetworkServiceAssociation","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "two statements, one of them conditioned",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":["vpc-lattice:CreateServiceNetworkVpcAssociation"],"Resource":"*","Condition":{"StringEquals":{"aws:PrincipalOrgID":"o-example"}}},{"Effect":"Deny","Principal":"*","Action":"vpc-lattice:DeleteServiceNetwork","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a document padded with whitespace",
            r#"   { "Version": "2012-10-17", "Statement": [ { "Effect": "Allow", "Principal": { "AWS": "210987654321" }, "Action": "vpc-lattice:GetService", "Resource": "*" } ] }   "#
                .to_string(),
        ),
        (
            "non-ASCII characters in a statement id",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"café-ünïcode","Effect":"Allow","Principal":"*","Action":"vpc-lattice:GetServiceNetwork","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a document just inside the PolicyString ceiling",
            {
                let head = r#"{"Version":"2012-10-17","Id":""#;
                let tail = r#"","Statement":[]}"#;
                let filler = POLICY_MAX_LENGTH - head.len() - tail.len();
                format!("{head}{}{tail}", "a".repeat(filler))
            },
        ),
    ]
}

/// The `policy` member a get reported, which must be the JSON string the put submitted.
fn reported_policy(case: &str, answer: &Answer) -> String {
    answer.body["policy"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the read must report the stored policy as a string, got {}",
                answer.body
            )
        })
        .to_string()
}

/// A read that reports no policy at all: the member the model declares optional is
/// absent, which is what a resource with no policy attached looks like.
fn assert_no_policy(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a resource with no policy is still a resource, expected 200, \
         got {} / {}",
        answer.status, answer.body
    );
    assert!(
        answer.body["policy"].is_null(),
        "case {case:?}: expected no policy member, got {}",
        answer.body
    );
}

/// Rule 1, over the document forms × the kinds a policy can be attached to here.
///
/// Each row: the put stores the document, the get reports it verbatim, an unrelated
/// change to the resource (and a policy on another resource) leaves it alone, a second
/// put replaces it, and the delete clears it — after which the get reports no policy and
/// a repeated delete is still accepted.
#[tokio::test]
async fn a_resource_policy_round_trips_survives_unrelated_changes_and_clears_on_delete() {
    let lattice = Lattice::new();

    let network = lattice
        .create_service_network(serde_json::json!({ "name": "policy-net" }))
        .await;
    assert_eq!(
        network.status, 201,
        "fixture service network: expected 201, got {} / {}",
        network.status, network.body
    );
    let service = lattice
        .create_service(serde_json::json!({ "name": "policy-svc" }))
        .await;
    assert_eq!(
        service.status, 201,
        "fixture service: expected 201, got {} / {}",
        service.status, service.body
    );
    let network_id = network.str_field("id");
    let network_arn = network.str_field("arn");
    let service_id = service.str_field("id");
    let service_arn = service.str_field("arn");

    // The resource a row attaches its policy to, and the "something else changed" step
    // that must not disturb it: for a service network, an `authType` update of the
    // network itself; for a service, a listener created under it.
    let documents = policy_documents();
    let targets = [
        ("service network", network_arn.clone(), network_id.clone()),
        ("service", service_arn.clone(), service_id.clone()),
    ];

    for (row, (form, document)) in documents.iter().enumerate() {
        for (kind, target_arn, target_id) in &targets {
            let case = format!("{kind}: {form}");

            let put = put_policy(&lattice, target_arn, document).await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: the put expected 200, got {} / {}",
                put.status, put.body
            );

            let read = get_policy(&lattice, target_arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the read expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                &reported_policy(&case, &read),
                document,
                "case {case:?}: the stored policy must be the submitted document verbatim"
            );

            // Something else changes: the resource's own members, and the OTHER
            // resource's policy. Neither may touch this one.
            let unrelated = if kind == &"service network" {
                lattice
                    .call(
                        "PATCH",
                        &format!("/servicenetworks/{target_id}"),
                        Some(serde_json::json!({ "authType": "AWS_IAM" })),
                    )
                    .await
            } else {
                lattice
                    .call(
                        "POST",
                        &format!("/services/{target_id}/listeners"),
                        Some(serde_json::json!({
                            "name": format!("policy-listener-{row}"),
                            "protocol": "HTTP",
                            "port": 8000 + i32::try_from(row).expect("the row index fits"),
                            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
                        })),
                    )
                    .await
            };
            assert!(
                (200..300).contains(&unrelated.status),
                "case {case:?}: the unrelated change expected a 2xx, got {} / {}",
                unrelated.status,
                unrelated.body
            );
            let other_arn = if kind == &"service network" {
                &service_arn
            } else {
                &network_arn
            };
            let other_document = format!(r#"{{"Version":"2012-10-17","Id":"other-{row}"}}"#);
            let other_put = put_policy(&lattice, other_arn, &other_document).await;
            assert_eq!(
                other_put.status, 200,
                "case {case:?}: the other resource's put expected 200, got {} / {}",
                other_put.status, other_put.body
            );

            let after = get_policy(&lattice, target_arn).await;
            assert_eq!(
                &reported_policy(&case, &after),
                document,
                "case {case:?}: the policy must survive a change to anything else"
            );
            let other_read = get_policy(&lattice, other_arn).await;
            assert_eq!(
                reported_policy(&case, &other_read),
                other_document,
                "case {case:?}: each resource keeps its OWN policy"
            );

            // A second put REPLACES the document rather than merging or appending.
            let replacement = format!(r#"{{"Version":"2012-10-17","Id":"replacement-{row}"}}"#);
            let replaced = put_policy(&lattice, target_arn, &replacement).await;
            assert_eq!(
                replaced.status, 200,
                "case {case:?}: the replacing put expected 200, got {} / {}",
                replaced.status, replaced.body
            );
            let reread = get_policy(&lattice, target_arn).await;
            assert_eq!(
                reported_policy(&case, &reread),
                replacement,
                "case {case:?}: the latest put is the policy the resource has"
            );

            // The delete clears it, and nothing else.
            let deleted = delete_policy(&lattice, target_arn).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: the delete expected 204, got {} / {}",
                deleted.status, deleted.body
            );
            assert_no_policy(&case, &get_policy(&lattice, target_arn).await);
            assert_eq!(
                reported_policy(&case, &get_policy(&lattice, other_arn).await),
                other_document,
                "case {case:?}: clearing one resource's policy leaves the other's"
            );

            // `DeleteResourcePolicy` is `@idempotent`: a resource that already has no
            // policy is accepted, not refused.
            let again = delete_policy(&lattice, target_arn).await;
            assert_eq!(
                again.status, 204,
                "case {case:?}: a repeated delete expected 204, got {} / {}",
                again.status, again.body
            );
            assert_no_policy(&case, &get_policy(&lattice, target_arn).await);

            // Leave the store as the next row expects it.
            let cleared = delete_policy(&lattice, other_arn).await;
            assert_eq!(
                cleared.status, 204,
                "case {case:?}: clearing the other resource expected 204, got {} / {}",
                cleared.status, cleared.body
            );
        }
    }
}

/// Rule 2's not-founds: an ARN of the modelled shape that names no persisted resource is
/// a 404 on every policy operation, with AWS's not-found construction over the kind the
/// ARN named and the ARN the caller submitted.
///
/// The rows span the three kinds `ResourceArn` enumerates — including the resource
/// configuration, which no implemented create can mint — and all three operations.
#[tokio::test]
async fn a_policy_call_naming_no_persisted_resource_is_the_modelled_not_found() {
    let lattice = Lattice::new();

    // (case, ARN, the CamelCase kind token AWS's message names, the resourceType token).
    // Both tokens are written out here rather than read from the service, so no row can
    // pass by agreeing with the code under test.
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "a service network that was never created",
            expected_arn(
                ARN_KIND_SERVICE_NETWORK,
                &absent_id(PREFIX_SERVICE_NETWORK, "0"),
            ),
            "ServiceNetwork",
            "SERVICE_NETWORK",
        ),
        (
            "a service that was never created",
            expected_arn(ARN_KIND_SERVICE, &absent_id(PREFIX_SERVICE, "z")),
            "Service",
            "SERVICE",
        ),
        (
            "a resource configuration, a kind nothing can create yet",
            expected_arn(
                ARN_KIND_RESOURCE_CONFIGURATION,
                &absent_id(PREFIX_RESOURCE_CONFIGURATION, "a1b2c3d4"),
            ),
            "ResourceConfiguration",
            "RESOURCE_CONFIGURATION",
        ),
    ];

    for (row, arn, label, resource_type) in rows {
        for (operation, answer) in [
            (
                "PutResourcePolicy",
                put_policy(&lattice, &arn, r#"{"Version":"2012-10-17"}"#).await,
            ),
            ("GetResourcePolicy", get_policy(&lattice, &arn).await),
            ("DeleteResourcePolicy", delete_policy(&lattice, &arn).await),
        ] {
            let case = format!("{operation}: {row}");
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
            assert_eq!(
                answer.body["message"].as_str(),
                Some(format!("{label} {arn} cannot be found.").as_str()),
                "case {case:?}: expected AWS's not-found construction, got {}",
                answer.body
            );
            assert_eq!(
                answer.body["resourceType"].as_str(),
                Some(resource_type),
                "case {case:?}: expected the kind's resourceType token, got {}",
                answer.body
            );
            assert_eq!(
                answer.body["resourceId"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: resourceId must name what the request named, got {}",
                answer.body
            );
        }
        // A refused put stores nothing, so the resource still has no policy to read.
        let read = get_policy(&lattice, &arn).await;
        assert_eq!(
            read.status, 404,
            "case {row:?}: a refused put must not create a resource, got {} / {}",
            read.status, read.body
        );
    }
}

/// Rule 2's modelled-constraint refusals: a `policy` outside `PolicyString`'s `@length`
/// or `@pattern`, and a `resourceArn` outside `ResourceArn`'s pattern, are 400s — and
/// none of them writes a policy onto the resource the request named.
#[tokio::test]
async fn a_policy_call_outside_a_modelled_constraint_is_400_and_writes_nothing() {
    let lattice = Lattice::new();
    let network = lattice
        .create_service_network(serde_json::json!({ "name": "constraint-net" }))
        .await;
    let network_id = network.str_field("id");
    let network_arn = network.str_field("arn");

    let stored = r#"{"Version":"2012-10-17","Id":"kept"}"#;
    let put = put_policy(&lattice, &network_arn, stored).await;
    assert_eq!(
        put.status, 200,
        "fixture put: expected 200, got {} / {}",
        put.status, put.body
    );

    // (case, the ARN the request names, the policy it submits)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a policy shorter than the modelled minimum",
            network_arn.clone(),
            String::new(),
        ),
        (
            "a policy of whitespace only, which the pattern forbids",
            network_arn.clone(),
            "   ".to_string(),
        ),
        (
            "a policy one character past the modelled ceiling",
            network_arn.clone(),
            format!(
                r#"{{"Id":"{}"}}"#,
                "b".repeat(POLICY_MAX_LENGTH - r#"{"Id":""#.len() - r#""}"#.len() + 1)
            ),
        ),
        (
            "the bare resource id, a form the ARN member does not model",
            network_id.clone(),
            stored.to_string(),
        ),
        (
            "an ARN whose kind segment and id prefix disagree",
            expected_arn(ARN_KIND_SERVICE, &absent_id(PREFIX_SERVICE_NETWORK, "0")),
            stored.to_string(),
        ),
    ];

    for (case, arn, policy) in rows {
        let answer = put_policy(&lattice, &arn, &policy).await;
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
        // The refusal wrote nothing: the policy the fixture put is still the one stored.
        let read = get_policy(&lattice, &network_arn).await;
        assert_eq!(
            reported_policy(case, &read),
            stored,
            "case {case:?}: a refused call must not change the stored policy"
        );
    }
}
