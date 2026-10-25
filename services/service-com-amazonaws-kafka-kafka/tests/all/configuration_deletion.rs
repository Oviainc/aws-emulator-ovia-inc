// SPDX-License-Identifier: CC0-1.0

//! `DeleteConfiguration`, driven end to end over the wire through the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request:
//! the rows differ along whatever the rule generalises over (the submitted
//! description, the Kafka versions named, the size and byte content of the
//! `server.properties` blob, how many revisions the configuration holds, how
//! many clusters use it, what makes an `Arn` resolve to nothing, and the signing
//! region). Where the rule is an invariant — a deleted configuration is gone
//! from every reader and its name is free again, a refused delete leaves the
//! registry exactly as it found it — the rows walk the whole space the rule
//! names.

use serde_json::{json, Value};

// The service's own refusal wordings and its configuration state values, so the
// suite and the handlers cannot drift apart on what the service says.
use service_com_amazonaws_kafka_kafka::state::{
    CONFIGURATION_ARN_NOT_FOUND_MESSAGE, CONFIGURATION_IN_USE_MESSAGE,
    CONFIGURATION_STATE_ACTIVE, CONFIGURATION_STATE_DELETING,
};

// The router harness, the signing region, the account id and the URI encoder
// every suite of this crate shares, so this one adds only its own rule and its
// configuration-shaped requests.
use super::kafka_harness::{
    configuration_rows, pct, sleep_past_activation, ClusterRequest, ConfigurationRequest, Emulator,
    Reply, ACCOUNT, REGION,
};

/// The table the suites below walk: the crate's shared configuration table —
/// whose rows differ along whether a description was given, how many Kafka
/// versions were named, and the size and byte content of the
/// `server.properties` blob — with one row carrying two further revisions, so
/// the delete is also exercised on a configuration that holds more than the
/// body its create submitted.
fn rows() -> Vec<ConfigurationRequest> {
    let mut rows = configuration_rows();
    if let Some(last) = rows.last_mut() {
        last.extra_revisions = vec![
            b"num.partitions=2\n".to_vec(),
            b"num.partitions=4\n".to_vec(),
        ];
    }
    rows
}

/// Create `request` under `region`'s endpoint, apply the revisions the row
/// carries, and hand back the minted ARN.
async fn create_ok_in(emulator: &Emulator, region: &str, request: &ConfigurationRequest) -> String {
    emulator.create_configuration_row_ok_in(region, request).await
}

/// `DeleteConfiguration`, which the model binds to
/// `DELETE /v1/configurations/{Arn}`.
async fn delete(emulator: &Emulator, arn: &str) -> Reply {
    delete_in(emulator, REGION, arn).await
}

/// The same delete, as an endpoint for `region` serves it.
async fn delete_in(emulator: &Emulator, region: &str, arn: &str) -> Reply {
    emulator
        .call_in(
            region,
            "DELETE",
            &format!("/v1/configurations/{}", pct(arn)),
            None,
        )
        .await
}

/// `DescribeConfiguration`, which the model binds to
/// `GET /v1/configurations/{Arn}`.
async fn describe_in(emulator: &Emulator, region: &str, arn: &str) -> Reply {
    emulator
        .call_in(
            region,
            "GET",
            &format!("/v1/configurations/{}", pct(arn)),
            None,
        )
        .await
}

/// `ListConfigurations`, which the model binds to `GET /v1/configurations`.
async fn list_in(emulator: &Emulator, region: &str) -> Reply {
    emulator
        .call_in(region, "GET", "/v1/configurations", None)
        .await
}

/// Whether `ListConfigurations` reported `arn` at `region`'s endpoint.
async fn is_listed(emulator: &Emulator, region: &str, arn: &str) -> bool {
    let listed = list_in(emulator, region).await;
    assert_eq!(
        listed.status, 200,
        "expected ListConfigurations in {region} to succeed, got {} {}",
        listed.status, listed.body
    );
    listed.body["configurations"]
        .as_array()
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.get("arn").and_then(Value::as_str) == Some(arn))
        })
}

/// Assert `reply` is the refusal the service answers an `Arn` it resolves to no
/// configuration with: HTTP 400 `BadRequestException`, naming `Arn` as the
/// parameter at fault.
fn assert_arn_resolves_to_nothing(reply: &Reply, case: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        CONFIGURATION_ARN_NOT_FOUND_MESSAGE,
        "case {case}: expected the unresolved-ARN wording, got {}",
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("Arn"),
        "case {case}: the refusal must name Arn as the parameter at fault, got {}",
        reply.body
    );
}

/// RULE: deleting a configuration takes it out of the service's registry and
/// reports it in the `DELETING` state under its own ARN, so no later reader
/// resolves or lists it and the name it held can be taken again.
#[tokio::test]
async fn a_deleted_configuration_is_gone_from_every_reader() {
    let emulator = Emulator::new();
    let rows = rows();

    let mut created: Vec<(ConfigurationRequest, String)> = Vec::new();
    for row in &rows {
        let arn = create_ok_in(&emulator, REGION, row).await;
        created.push((row.clone(), arn));
    }

    for (row, arn) in &created {
        // Readable before the delete, so what follows is a change this call
        // made rather than a configuration that was never there.
        let before = describe_in(&emulator, REGION, arn).await;
        assert_eq!(
            (before.status, before.body["state"].as_str()),
            (200, Some(CONFIGURATION_STATE_ACTIVE)),
            "case {row:?}: the configuration must be ACTIVE before the delete, got {} {}",
            before.status,
            before.body
        );

        let deleted = delete(&emulator, arn).await;
        assert_eq!(
            (deleted.status, deleted.error_code()),
            (200, ""),
            "case {row:?}: expected DeleteConfiguration to succeed, got {} {}",
            deleted.status,
            deleted.body
        );
        assert_eq!(
            deleted.body["arn"].as_str(),
            Some(arn.as_str()),
            "case {row:?}: the delete must report the configuration's own ARN, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.body["state"].as_str(),
            Some(CONFIGURATION_STATE_DELETING),
            "case {row:?}: a deleted configuration is reported DELETING, got {}",
            deleted.body
        );

        // Gone from the single-resource reader, from the listing, and from the
        // revision reader: one removal, every reader.
        let described = describe_in(&emulator, REGION, arn).await;
        assert_eq!(
            described.status, 404,
            "case {row:?}: a deleted configuration must not resolve, got {} {}",
            described.status, described.body
        );
        assert!(
            !is_listed(&emulator, REGION, arn).await,
            "case {row:?}: {arn} must not be listed after its delete"
        );
        let revision = emulator.describe_configuration_revision(arn, 1).await;
        assert_eq!(
            revision.status, 404,
            "case {row:?}: a deleted configuration holds no readable revision, got {} {}",
            revision.status, revision.body
        );

        // The name it held is free again: a create may take it, and the new
        // configuration is a DIFFERENT resource with its own ARN and its own
        // revision history, whatever the deleted one had accumulated.
        let remade_row = ConfigurationRequest {
            extra_revisions: Vec::new(),
            ..row.clone()
        };
        let remade = create_ok_in(&emulator, REGION, &remade_row).await;
        assert_ne!(
            &remade, arn,
            "case {row:?}: a configuration created after the delete must get its own ARN"
        );
        let remade_described = describe_in(&emulator, REGION, &remade).await;
        assert_eq!(
            (
                remade_described.status,
                remade_described.body["state"].as_str()
            ),
            (200, Some(CONFIGURATION_STATE_ACTIVE)),
            "case {row:?}: the re-created configuration must be readable and ACTIVE, got {} {}",
            remade_described.status,
            remade_described.body
        );
        assert_eq!(
            remade_described.body["latestRevision"]["revision"].as_i64(),
            Some(1),
            "case {row:?}: the re-created configuration starts its own revision history, got {}",
            remade_described.body
        );
    }
}

/// RULE: an `Arn` an endpoint resolves to no configuration of its own region is
/// refused HTTP 400 `BadRequestException` and deletes nothing — whether it is
/// not a configuration ARN at all, names a configuration another region holds,
/// or names one a delete has already taken.
#[tokio::test]
async fn an_arn_that_names_no_configuration_is_refused() {
    let emulator = Emulator::new();

    // One configuration this endpoint holds, one that lives in another region,
    // and one this endpoint held until it was deleted.
    let held = create_ok_in(
        &emulator,
        REGION,
        &ConfigurationRequest::named("held-here"),
    )
    .await;
    let elsewhere = create_ok_in(
        &emulator,
        "eu-central-1",
        &ConfigurationRequest::named("held-elsewhere"),
    )
    .await;
    let already_deleted = create_ok_in(
        &emulator,
        REGION,
        &ConfigurationRequest::named("already-deleted"),
    )
    .await;
    let first = delete(&emulator, &already_deleted).await;
    assert_eq!(
        first.status, 200,
        "expected the first delete to succeed, got {} {}",
        first.status, first.body
    );

    // A well-formed configuration ARN of this region that was never minted, and
    // the ARNs of other MSK resource kinds, which name no configuration however
    // well formed they are.
    let never_minted = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/never-minted/\
         11111111-2222-3333-4444-555555555555-1"
    );
    let cluster_arn = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/a-cluster/\
         11111111-2222-3333-4444-555555555555-2"
    );
    let replicator_arn = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:replicator/a-replicator/\
         11111111-2222-3333-4444-555555555555-3"
    );
    let other_account = held.replace(ACCOUNT, "123456789012");
    let other_service = held.replace(":kafka:", ":kafka-cluster:");
    let wrong_region = held.replace(REGION, "eu-central-1");

    let refused: Vec<(&str, String)> = vec![
        ("empty-ish-arn", "not-an-arn".to_string()),
        ("truncated-arn", "arn:aws:kafka".to_string()),
        ("cluster-arn", cluster_arn),
        ("replicator-arn", replicator_arn),
        ("never-minted-configuration", never_minted),
        ("another-account", other_account),
        ("another-service-namespace", other_service),
        // Well formed and really registered — in ANOTHER region, which this
        // endpoint does not serve.
        ("another-regions-configuration", elsewhere.clone()),
        ("this-regions-arn-with-another-region", wrong_region),
        // Registered once, taken by the delete above.
        ("already-deleted", already_deleted.clone()),
    ];

    for (case, arn) in &refused {
        let reply = delete(&emulator, arn).await;
        assert_arn_resolves_to_nothing(&reply, case);
    }

    // Nothing above deleted anything: the configuration this endpoint holds is
    // still readable here, and the one in the other region is still readable
    // there and still deletable there.
    let described = describe_in(&emulator, REGION, &held).await;
    assert_eq!(
        described.status, 200,
        "the stored configuration must still resolve after the refusals, got {} {}",
        described.status, described.body
    );
    assert!(
        is_listed(&emulator, "eu-central-1", &elsewhere).await,
        "the other region's configuration must still be listed there"
    );
    let deleted_there = delete_in(&emulator, "eu-central-1", &elsewhere).await;
    assert_eq!(
        (deleted_there.status, deleted_there.body["state"].as_str()),
        (200, Some(CONFIGURATION_STATE_DELETING)),
        "the region that holds the configuration must be able to delete it, got {} {}",
        deleted_there.status,
        deleted_there.body
    );
}

/// RULE: a configuration a cluster was created with is in use by that cluster,
/// so deleting it is refused HTTP 400 `BadRequestException` and leaves the
/// configuration readable and `ACTIVE`; once no cluster uses it any more, the
/// same delete succeeds.
#[tokio::test]
async fn a_configuration_a_cluster_uses_cannot_be_deleted() {
    let emulator = Emulator::new();

    // The rows differ along how many clusters use the configuration: one, and
    // several — the refusal holds while ANY of them does.
    for users in [1_usize, 3] {
        let configuration = create_ok_in(
            &emulator,
            REGION,
            &ConfigurationRequest::named(&format!("used-by-{users}")),
        )
        .await;
        // A second configuration nothing refers to, created in the same
        // registry: the refusal must be about the ONE configuration in use.
        let unused = create_ok_in(
            &emulator,
            REGION,
            &ConfigurationRequest::named(&format!("unused-beside-{users}")),
        )
        .await;

        let mut clusters = Vec::new();
        for index in 0..users {
            let mut body = ClusterRequest::named(&format!("user-{users}-{index}")).to_json();
            body["configurationInfo"] = json!({ "arn": configuration, "revision": 1 });
            let created = emulator.call("POST", "/v1/clusters", Some(body)).await;
            assert_eq!(
                created.status, 200,
                "case users={users}: expected CreateCluster to succeed, got {} {}",
                created.status, created.body
            );
            clusters.push(
                created.body["clusterArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case users={users}: CreateCluster returned no clusterArn")
                    })
                    .to_string(),
            );
        }
        // A cluster is deletable once it has finished provisioning, so this row's
        // clusters are waited out of `CREATING` before any of them is deleted
        // below.
        sleep_past_activation();

        // Refused while every one of them uses it, and refused again after each
        // user but the last is gone.
        for remaining in (1..=users).rev() {
            let reply = delete(&emulator, &configuration).await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException"),
                "case users={users}, remaining={remaining}: expected the in-use refusal, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.error_message(),
                CONFIGURATION_IN_USE_MESSAGE,
                "case users={users}, remaining={remaining}: expected the in-use wording, got {}",
                reply.body
            );
            // The refused delete changed nothing: the configuration is still
            // readable, still ACTIVE, and still listed.
            let described = describe_in(&emulator, REGION, &configuration).await;
            assert_eq!(
                (described.status, described.body["state"].as_str()),
                (200, Some(CONFIGURATION_STATE_ACTIVE)),
                "case users={users}, remaining={remaining}: a refused delete leaves the \
                 configuration ACTIVE, got {} {}",
                described.status,
                described.body
            );
            assert!(
                is_listed(&emulator, REGION, &configuration).await,
                "case users={users}, remaining={remaining}: a refused delete leaves it listed"
            );
            // The configuration nothing refers to is deletable throughout: it
            // is the association that refuses, not the presence of clusters.
            if remaining == users {
                let unused_reply = delete(&emulator, &unused).await;
                assert_eq!(
                    (unused_reply.status, unused_reply.body["state"].as_str()),
                    (200, Some(CONFIGURATION_STATE_DELETING)),
                    "case users={users}: a configuration no cluster uses is deletable, got {} {}",
                    unused_reply.status,
                    unused_reply.body
                );
            }

            let cluster = clusters.pop().expect("a cluster that uses the configuration");
            let removed = emulator
                .call("DELETE", &format!("/v1/clusters/{}", pct(&cluster)), None)
                .await;
            assert_eq!(
                removed.status, 200,
                "case users={users}: expected DeleteCluster to succeed, got {} {}",
                removed.status, removed.body
            );
        }

        // No cluster uses it any more, so the same call now succeeds.
        let reply = delete(&emulator, &configuration).await;
        assert_eq!(
            (reply.status, reply.body["state"].as_str()),
            (200, Some(CONFIGURATION_STATE_DELETING)),
            "case users={users}: the delete must succeed once nothing uses it, got {} {}",
            reply.status,
            reply.body
        );
        assert!(
            !is_listed(&emulator, REGION, &configuration).await,
            "case users={users}: the deleted configuration must not be listed"
        );
    }
}
