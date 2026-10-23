// SPDX-License-Identifier: CC0-1.0

//! `DeleteSecurityConfiguration`, end to end: a delete that is actually observable.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the shared harness
//! [`crate::virtual_cluster_job_run_loop::Emr`] and driven with restJson1 HTTP requests,
//! so routing, deserialization, constraint validation, the handler and serialization all
//! run exactly as they do for an AWS SDK client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. A successful delete REMOVES the resource from state, so the id it reports stops
//!    resolving: every later read of that id answers the modelled
//!    `ResourceNotFoundException` (HTTP 400, `DescribeSecurityConfiguration`'s only
//!    not-found error) no matter how many times it is polled, and never a 2xx body.
//! 2. A delete names exactly one resource: the configurations it did not name stay
//!    readable, whatever their number, content or position in the store.
//! 3. `DeleteSecurityConfiguration` models no not-found error and reports the id it
//!    deleted, so an id the store does not hold — including a second delete of an id that
//!    was already deleted, and the id of a resource of another type — is refused with the
//!    modelled `ValidationException` (HTTP 400) and changes nothing.
//! 4. A delete frees the slot it held: after the store has been filled to its retention
//!    cap, a delete makes room for exactly one more create.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr};

/// `POST /securityconfigurations` — `CreateSecurityConfiguration`.
async fn create_configuration(emr: &Emr, body: Value) -> Answer {
    emr.send("POST", "/securityconfigurations", Some(body)).await
}

/// `GET /securityconfigurations/{id}` — `DescribeSecurityConfiguration`.
async fn describe_configuration(emr: &Emr, id: &str) -> Answer {
    emr.send("GET", &format!("/securityconfigurations/{id}"), None)
        .await
}

/// `DELETE /securityconfigurations/{id}` — `DeleteSecurityConfiguration`.
async fn delete_configuration(emr: &Emr, id: &str) -> Answer {
    emr.send("DELETE", &format!("/securityconfigurations/{id}"), None)
        .await
}

/// The smallest body `CreateSecurityConfiguration` accepts: `name`, `clientToken` and
/// `securityConfigurationData` are all `@required` (smithy_query: operation
/// `CreateSecurityConfiguration`).
fn plain_body(name: &str, token: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "securityConfigurationData": {},
    })
}

/// Create one configuration from `body` and return the id it minted.
async fn created_id(emr: &Emr, case: &str, body: Value) -> String {
    let answer = create_configuration(emr, body).await;
    text(answer.ok(case), "/id", case)
}

/// The id a describe answered for this configuration, so a read is checked to be the
/// record that was asked for and not somebody else's.
fn described_id(answer: &Answer, case: &str) -> String {
    let configuration = field(answer.ok(case), "/securityConfiguration", case);
    text(configuration, "/id", case)
}

/// One `CreateSecurityConfiguration` body to delete.
struct DeleteCase {
    label: &'static str,
    body: Value,
}

/// RULES 1 and 3: a delete removes the configuration, so the read that follows it misses
/// — for as many polls as a caller cares to make — and a second delete of the same id is
/// refused rather than answered as a no-op.
///
/// The rows differ along everything the delete generalises over: how much of
/// `securityConfigurationData` the record carries and how deeply, whether it has tags or a
/// `containerProvider`, and the form and length of its name and token. None of that is
/// supposed to make any difference to whether the delete takes effect, which is the point.
#[tokio::test]
async fn a_deleted_security_configuration_stops_resolving() {
    let cases = [
        DeleteCase {
            label: "only the required members",
            body: plain_body("minimal", "del-tok-minimal"),
        },
        DeleteCase {
            label: "lake formation, a container provider and tags",
            body: json!({
                "name": "lake.formation_1",
                "clientToken": "del-tok-lake",
                "containerProvider": {
                    "id": "eks-lake",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "emr-lake"}},
                },
                "securityConfigurationData": {
                    "authorizationConfiguration": {
                        "lakeFormationConfiguration": {
                            "authorizedSessionTagValue": "Emr Session Tag",
                            "secureNamespaceInfo": {
                                "clusterId": "eks-secure-1",
                                "namespace": "emr-secure",
                            },
                        },
                    },
                },
                "tags": {"team": "data", "env": "test"},
            }),
        },
        DeleteCase {
            label: "identity-center authentication, no tags",
            body: json!({
                "name": "identity-center",
                "clientToken": "del/tok.idc-1",
                "securityConfigurationData": {
                    "authenticationConfiguration": {
                        "identityCenterConfiguration": {
                            "enableIdentityCenter": true,
                            "identityCenterInstanceARN":
                                "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                        },
                    },
                },
            }),
        },
        DeleteCase {
            label: "at the name and token length bounds",
            // `ResourceNameString` is len 1..64 and `ClientToken` len 1..64: the upper
            // bound of each (smithy_query: operation `CreateSecurityConfiguration`).
            body: plain_body(&"n".repeat(64), &"t".repeat(64)),
        },
        DeleteCase {
            label: "at the name and token lower length bound",
            body: plain_body("n", "t"),
        },
    ];

    for case in &cases {
        let label = case.label;
        // One emulator per row, so each row's store holds only its own configuration.
        let emr = Emr::new();
        let id = created_id(&emr, label, case.body.clone()).await;
        assert_eq!(
            described_id(&describe_configuration(&emr, &id).await, label),
            id,
            "case {label:?}: the configuration must describe before it is deleted"
        );

        let deleted = delete_configuration(&emr, &id).await;
        assert_eq!(
            text(deleted.ok(label), "/id", label),
            id,
            "case {label:?}: the delete must report the id it deleted"
        );

        // RULE 1: the read never comes back. A caller polling for the resource to go
        // away is the only confirmation a delete has, so poll it: no attempt may answer
        // 2xx, and each must carry the modelled error code at HTTP 400.
        for attempt in 1..=4 {
            let read = describe_configuration(&emr, &id).await;
            read.refused(
                &format!("{label}: poll {attempt} of the deleted configuration"),
                400,
                "ResourceNotFoundException",
            );
        }

        // RULE 3: the id is not there to delete a second time.
        delete_configuration(&emr, &id).await.refused(
            &format!("{label}: deleting the same id twice"),
            400,
            "ValidationException",
        );
    }
}

/// RULE 2: a delete names ONE resource. The rows differ in how many configurations the
/// store holds and which position the deleted one occupies, so a delete that removed too
/// much (or the wrong record) is caught whichever slot it hit.
#[tokio::test]
async fn deleting_one_configuration_leaves_every_other_readable() {
    // (label, how many configurations the store holds, which of them to delete)
    let cases = [
        ("the only configuration", 1usize, 0usize),
        ("the first of two", 2, 0),
        ("the second of two", 2, 1),
        ("the middle of five", 5, 2),
        ("the last of five", 5, 4),
        ("the first of five", 5, 0),
    ];

    for (label, count, target) in cases {
        let emr = Emr::new();
        let mut ids = Vec::new();
        for index in 0..count {
            ids.push(
                created_id(
                    &emr,
                    label,
                    plain_body(&format!("set-{index}"), &format!("del-set-tok-{index}")),
                )
                .await,
            );
        }
        assert_eq!(
            ids.iter().collect::<BTreeSet<_>>().len(),
            count,
            "case {label:?}: every configuration must have its own id, got {ids:?}"
        );

        let deleted = delete_configuration(&emr, &ids[target]).await;
        assert_eq!(
            text(deleted.ok(label), "/id", label),
            ids[target],
            "case {label:?}: the delete must report the id it deleted"
        );

        for (index, id) in ids.iter().enumerate() {
            if index == target {
                describe_configuration(&emr, id).await.refused(
                    &format!("{label}: the deleted configuration {index}"),
                    400,
                    "ResourceNotFoundException",
                );
            } else {
                assert_eq!(
                    &described_id(
                        &describe_configuration(&emr, id).await,
                        &format!("{label}: the surviving configuration {index}"),
                    ),
                    id,
                    "case {label:?}: configuration {index} must still describe after the delete"
                );
            }
        }
    }
}

/// RULE 3: `DeleteSecurityConfiguration` resolves its id out of state and reports the id
/// it deleted, so an id the store does not hold is refused with the modelled
/// `ValidationException` (the operation models no not-found error: smithy_query, operation
/// `DeleteSecurityConfiguration`, errors `InternalServerException` 500 /
/// `ValidationException` 400) — and the refusal deletes nothing.
///
/// The rows span the forms a `ResourceIdString` takes — both published length bounds, an
/// all-digit id, a plausible minted-width id — plus the id of a resource of a DIFFERENT
/// type, which is well-formed but is not a security configuration.
#[tokio::test]
async fn deleting_an_unknown_security_configuration_id_is_refused() {
    let emr = Emr::new();
    let present = created_id(
        &emr,
        "the stored configuration",
        plain_body("present", "del-tok-present"),
    )
    .await;
    // A virtual cluster id: another resource type's id, so it names no security
    // configuration even though it is a well-formed `ResourceIdString`.
    let cluster = emr.cluster("del-cluster", "del-tok-cluster").await;

    let cases = [
        // `ResourceIdString` is `@length 1..64`, `^[0-9a-z]+$` (smithy_query: operation
        // `DeleteSecurityConfiguration`, input `id`): both bounds, and inside them.
        ("the shortest id the model allows", "a".to_string()),
        ("the longest id the model allows", "z".repeat(64)),
        ("an all-digit id", "0123456789".to_string()),
        ("a plausible minted id", "doesnotexist1".to_string()),
        ("a virtual cluster's id", cluster.clone()),
    ];

    for (label, id) in &cases {
        assert_ne!(
            id, &present,
            "case {label:?}: the row must name an id no configuration was minted for"
        );
        delete_configuration(&emr, id)
            .await
            .refused(label, 400, "ValidationException");
    }

    // The refusals deleted nothing: the stored configuration still describes, and the
    // virtual cluster whose id was offered is still there too.
    assert_eq!(
        described_id(
            &describe_configuration(&emr, &present).await,
            "the stored configuration"
        ),
        present,
        "the stored configuration must survive every refused delete"
    );
    emr.send("GET", &format!("/virtualclusters/{cluster}"), None)
        .await
        .ok("the virtual cluster whose id was offered to the delete");
}

/// RULE 4: a delete frees the slot its record held. The store is capped at
/// [`state::MAX_SECURITY_CONFIGURATIONS`] and a security configuration has no retired form
/// to reclaim, so once the cap is reached only a delete can make room — and it makes room
/// for exactly one more.
#[tokio::test]
async fn a_delete_frees_one_security_configuration_slot() {
    let emr = Emr::new();
    let case = "filling the retention cap";
    let mut ids = Vec::new();
    for index in 0..state::MAX_SECURITY_CONFIGURATIONS {
        ids.push(
            created_id(
                &emr,
                case,
                plain_body(&format!("cap-{index}"), &format!("del-cap-tok-{index}")),
            )
            .await,
        );
    }
    create_configuration(&emr, plain_body("over", "del-cap-tok-over"))
        .await
        .refused("a create at the cap", 400, "ValidationException");

    // Free one slot, and exactly one create fits.
    let freed = ids[state::MAX_SECURITY_CONFIGURATIONS / 2].clone();
    delete_configuration(&emr, &freed)
        .await
        .ok("freeing one slot");
    let replacement = created_id(&emr, "the create that fits", plain_body("fits", "del-fits")).await;
    assert_ne!(
        replacement, freed,
        "the create after the delete must mint its own id, not reuse the deleted one"
    );
    assert_eq!(
        described_id(
            &describe_configuration(&emr, &replacement).await,
            "the create that fits"
        ),
        replacement,
        "the configuration created in the freed slot must describe"
    );
    describe_configuration(&emr, &freed).await.refused(
        "the configuration whose slot was freed",
        400,
        "ResourceNotFoundException",
    );
    create_configuration(&emr, plain_body("over-again", "del-cap-tok-over-again"))
        .await
        .refused("a create once the freed slot is taken", 400, "ValidationException");
}
