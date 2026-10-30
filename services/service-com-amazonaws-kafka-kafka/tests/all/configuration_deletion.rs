// SPDX-License-Identifier: CC0-1.0

//! `DeleteConfiguration`, driven end to end over the wire through the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request:
//! the rows differ along whatever the rule generalises over (the submitted
//! description, the Kafka versions named, the size and byte content of the
//! `server.properties` blob, how many revisions the configuration holds, how
//! many clusters use it, what makes an `Arn` resolve to nothing, and the signing
//! region). Where the rule is an invariant — a delete is accepted, reported
//! `DELETING` and completed by the clock, after which the configuration is gone
//! from every reader and its name is free again; a refused delete leaves the
//! registry exactly as it found it — the rows walk the whole space the rule
//! names.

use serde_json::{json, Value};

// The service's own refusal wording and its configuration state values, so the
// suite and the handlers cannot drift apart on what the service says.
use service_com_amazonaws_kafka_kafka::state::{
    CONFIGURATION_IN_USE_MESSAGE, CONFIGURATION_STATE_ACTIVE, CONFIGURATION_STATE_DELETING,
};

// The router harness, the signing region, the account id, the URI encoder and
// the configuration family's shared refusal assertions every suite of this crate
// shares, so this one adds only its own rule and its configuration-shaped
// requests.
use super::kafka_harness::{
    assert_configuration_arn_resolves_to_nothing, assert_configuration_state_refusal,
    configuration_rows, pct, sleep_past_activation, sleep_past_configuration_deletion,
    ClusterRequest, ConfigurationRequest, Emulator, Reply, ACCOUNT, REGION,
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

/// Whether `ListConfigurations` reported `arn` at `region`'s endpoint, and with
/// which `State` — `None` when the listing does not report it at all.
async fn listed_state(emulator: &Emulator, region: &str, arn: &str) -> Option<String> {
    let listed = list_in(emulator, region).await;
    assert_eq!(
        listed.status, 200,
        "expected ListConfigurations in {region} to succeed, got {} {}",
        listed.status, listed.body
    );
    listed.body["configurations"]
        .as_array()?
        .iter()
        .find(|entry| entry.get("arn").and_then(Value::as_str) == Some(arn))
        .map(|entry| {
            entry
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
}

/// Whether `ListConfigurations` reported `arn` at `region`'s endpoint.
async fn is_listed(emulator: &Emulator, region: &str, arn: &str) -> bool {
    listed_state(emulator, region, arn).await.is_some()
}

/// RULE: a configuration delete is ASYNCHRONOUS. It is accepted on the record:
/// the response reports the configuration's own ARN in state `DELETING`, the
/// configuration stays a resource every reader resolves and the listing
/// enumerates in that state, and a call that would CHANGE it while it is in that
/// state is refused with the state-machine refusal and changes nothing. Once the
/// clock owes the removal the configuration is gone from every reader and the
/// name it held can be taken again.
#[tokio::test]
async fn a_delete_is_accepted_as_deleting_and_completed_by_the_clock() {
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
        let revisions_before = emulator
            .describe_configuration(arn)
            .await
            .at("/latestRevision/revision")
            .as_i64();

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

        // The delete is in flight, so the configuration is STILL a resource of
        // this endpoint: both readers resolve it, in state DELETING.
        let described = describe_in(&emulator, REGION, arn).await;
        assert_eq!(
            (described.status, described.body["state"].as_str()),
            (200, Some(CONFIGURATION_STATE_DELETING)),
            "case {row:?}: a configuration whose delete is in flight is readable and \
             DELETING, got {} {}",
            described.status,
            described.body
        );
        assert_eq!(
            listed_state(&emulator, REGION, arn).await.as_deref(),
            Some(CONFIGURATION_STATE_DELETING),
            "case {row:?}: {arn} must still be listed, with state DELETING"
        );
        // Its revisions are still readable too: the record is still there.
        let revision = emulator.describe_configuration_revision(arn, 1).await;
        assert_eq!(
            revision.status, 200,
            "case {row:?}: revision 1 of a DELETING configuration is still readable, got {} {}",
            revision.status, revision.body
        );

        // A call that CHANGES it is refused on the state, not reported as a
        // lookup miss — and writes no revision.
        let refused = emulator
            .update_configuration(arn, None, b"num.partitions=9\n")
            .await;
        assert_configuration_state_refusal(&refused, &format!("{row:?} update-while-deleting"));
        let second_delete = delete(&emulator, arn).await;
        assert_configuration_state_refusal(
            &second_delete,
            &format!("{row:?} delete-while-deleting"),
        );
        assert_eq!(
            describe_in(&emulator, REGION, arn)
                .await
                .at("/latestRevision/revision")
                .as_i64(),
            revisions_before,
            "case {row:?}: a refused update must write no revision"
        );
    }

    // The clock owes every one of those deletions its completion by now.
    sleep_past_configuration_deletion();

    for (row, arn) in &created {
        // Gone from the single-resource reader, from the listing, and from the
        // revision reader: one removal, every reader. A reader that cannot
        // resolve the ARN answers the configuration family's
        // unresolvable-reference refusal, which is HTTP 400.
        let described = describe_in(&emulator, REGION, arn).await;
        assert_configuration_arn_resolves_to_nothing(
            &described,
            &format!("{row:?} deleted-configuration-read"),
            arn,
            REGION,
        );
        assert!(
            !is_listed(&emulator, REGION, arn).await,
            "case {row:?}: {arn} must not be listed once its delete has completed"
        );
        let revision = emulator.describe_configuration_revision(arn, 1).await;
        assert_configuration_arn_resolves_to_nothing(
            &revision,
            &format!("{row:?} deleted-configuration-revision-read"),
            arn,
            REGION,
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

/// RULE: while a configuration's delete is in flight its NAME is still taken —
/// it is still a configuration of this region — and only once the clock has
/// completed the removal may a create take that name again.
#[tokio::test]
async fn a_deleting_configuration_still_holds_its_name() {
    let emulator = Emulator::new();

    // The rows differ along the request the name is re-created from: the same
    // row, and a row carrying different members under that one name.
    let rows: Vec<(&str, ConfigurationRequest)> = vec![
        ("same-request", ConfigurationRequest::named("contested-1")),
        (
            "different-members",
            ConfigurationRequest {
                description: Some("a second take on the name".to_string()),
                kafka_versions: vec!["3.6.0".to_string()],
                server_properties: b"num.partitions=3\n".to_vec(),
                ..ConfigurationRequest::named("contested-2")
            },
        ),
    ];

    for (case, row) in &rows {
        let arn = create_ok_in(&emulator, REGION, row).await;
        let deleted = delete(&emulator, &arn).await;
        assert_eq!(
            (deleted.status, deleted.body["state"].as_str()),
            (200, Some(CONFIGURATION_STATE_DELETING)),
            "case {case}: expected the delete to be accepted, got {} {}",
            deleted.status,
            deleted.body
        );

        // The delete is in flight, so the name is still in use.
        let contested = emulator
            .call_in(REGION, "POST", "/v1/configurations", Some(row.to_json()))
            .await;
        assert_eq!(
            (contested.status, contested.error_code()),
            (409, "ConflictException"),
            "case {case}: a DELETING configuration still holds its name, got {} {}",
            contested.status,
            contested.body
        );

        sleep_past_configuration_deletion();

        // The removal is done, so the name is free and the create succeeds.
        let remade = create_ok_in(&emulator, REGION, row).await;
        assert_ne!(
            remade, arn,
            "case {case}: the re-created configuration must get its own ARN"
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
    // The delete is asynchronous, so the row below means "deleted AND gone":
    // waited out here, since a configuration still being deleted is a resource
    // this endpoint holds and is refused on its state instead.
    sleep_past_configuration_deletion();

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
        assert_configuration_arn_resolves_to_nothing(&reply, case, arn, REGION);
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

        // No cluster uses it any more, so the same call now succeeds: the
        // delete is accepted and the configuration is reported DELETING while
        // the removal is in flight, then leaves the listing once the clock owes
        // it.
        let reply = delete(&emulator, &configuration).await;
        assert_eq!(
            (reply.status, reply.body["state"].as_str()),
            (200, Some(CONFIGURATION_STATE_DELETING)),
            "case users={users}: the delete must succeed once nothing uses it, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            listed_state(&emulator, REGION, &configuration)
                .await
                .as_deref(),
            Some(CONFIGURATION_STATE_DELETING),
            "case users={users}: the accepted delete leaves it listed as DELETING"
        );
        sleep_past_configuration_deletion();
        assert!(
            !is_listed(&emulator, REGION, &configuration).await,
            "case users={users}: the deleted configuration must not be listed once the \
             removal has come due"
        );
    }
}
