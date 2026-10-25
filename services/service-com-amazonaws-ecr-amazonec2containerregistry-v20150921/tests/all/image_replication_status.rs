// SPDX-License-Identifier: CC0-1.0

//! `DescribeImageReplicationStatus`, end to end over the wire.
//!
//! The rule under test: a push stores one replication record per destination of every
//! replication rule whose repository filters cover the repository pushed to, and
//! `DescribeImageReplicationStatus` reports one status per record — `IN_PROGRESS` until the
//! clock makes that copy's completion due ([`state::IMAGE_REPLICATION_DELAY`]) and
//! `COMPLETE` afterwards. The set is decided at the push, so a rule stored afterwards adds
//! no status to an image the registry already held, and an image pushed while no rule
//! covered its repository reports the empty list. Reading changes nothing.
//!
//! The wire harness is `crate::wire`'s: every case goes through the router, so it exercises
//! routing, deserialization, the handlers, the persisted state and serialization.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{at, emulator, given_layer, ok, REGISTRY_ID};

use ecr::state;

/// Long enough past [`state::IMAGE_REPLICATION_DELAY`] that the completion the clock owes
/// is certainly due. Derived from the span itself, never a bare number.
const PAST_THE_DELAY: Duration =
    state::IMAGE_REPLICATION_DELAY.saturating_add(Duration::from_millis(150));

/// Another account's registry: a well-formed `RegistryId` (pattern `^[0-9]{12}$`) that is
/// not this one, so a destination naming it is cross-account replication.
const FOREIGN_REGISTRY_ID: &str = "111122223333";

/// Destination Regions, none of them the Region the harness signs for (`crate::wire`'s
/// `REGION`): a registry cannot replicate to itself, so the put refuses a destination in
/// the source Region.
const FIRST_REGION: &str = "us-east-2";
/// See [`FIRST_REGION`].
const SECOND_REGION: &str = "ap-south-1";

/// One row of the replication table: what the row is about, the repository it pushes into,
/// the rules in force at that push, and the `(Region, registry)` destinations expected.
type ReplicationRow<'a> = (&'a str, &'a str, Value, Vec<(&'a str, &'a str)>);

/// One replication destination.
fn destination(registry_id: &str, region: &str) -> Value {
    json!({"region": region, "registryId": registry_id})
}

/// One `PREFIX_MATCH` repository filter: "the repository name prefix to configure
/// replication for" (`RepositoryFilter$filter`).
fn prefix(filter: &str) -> Value {
    json!({"filter": filter, "filterType": "PREFIX_MATCH"})
}

/// Store `rules` as the registry's whole replication configuration.
async fn given_configuration(router: &MultiServiceRouter, case: &str, rules: Value) {
    ok(
        router,
        case,
        "PutReplicationConfiguration",
        &json!({"replicationConfiguration": {"rules": rules}}),
    )
    .await;
}

/// Create `repository` and push one single-layer image into it, answering the digest the
/// registry addresses it by.
async fn given_pushed_image(router: &MultiServiceRouter, case: &str, repository: &str) -> String {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let content = format!("{repository}-layer").into_bytes();
    let layer = given_layer(router, case, repository, &content).await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": layer,
            "size": content.len(),
        }],
    })
    .to_string();
    ok(
        router,
        case,
        "PutImage",
        &json!({"repositoryName": repository, "imageManifest": manifest}),
    )
    .await;
    state::manifest_digest(&manifest)
}

/// The `replicationStatuses` reported for one image, each entry reduced to the destination
/// it names and the status it is in, sorted so the comparison does not depend on the order
/// the rules happened to be stored in.
async fn statuses(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
) -> Vec<(String, String, String)> {
    let answer = ok(
        router,
        case,
        "DescribeImageReplicationStatus",
        &json!({"repositoryName": repository, "imageId": {"imageDigest": digest}}),
    )
    .await;
    assert_eq!(
        at(&answer, "/imageId/imageDigest"),
        json!(digest),
        "case {case:?}: the read reports the image it was asked about, got {answer:?}"
    );
    assert_eq!(
        at(&answer, "/repositoryName"),
        json!(repository),
        "case {case:?}: the read names the repository it was asked about, got {answer:?}"
    );
    let mut reported: Vec<(String, String, String)> = answer["replicationStatuses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                at(entry, "/region").as_str().unwrap_or_default().to_string(),
                at(entry, "/registryId")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                at(entry, "/status").as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    reported.sort();
    reported
}

/// A push under an active replication rule gains one status per destination, and each one
/// settles on the clock.
///
/// The rows differ along what the rule generalises over: whether the rule carries a
/// repository filter at all, whether its prefix covers the repository, how many
/// destinations the configuration names, whether two rules name the same destination, and
/// whether the destination registry is this account's or another's. The report's own case is
/// the single `PREFIX_MATCH` rule covering the repository.
#[tokio::test]
async fn a_push_under_a_replication_rule_reports_one_status_per_destination() {
    // (case, repository, the rules in force at the push, the destinations expected)
    let rows: Vec<ReplicationRow<'_>> = vec![
        (
            "a prefix filter covering the repository",
            "replicated-prod",
            json!([{
                "destinations": [destination(REGISTRY_ID, FIRST_REGION)],
                "repositoryFilters": [prefix("replicated-")],
            }]),
            vec![(FIRST_REGION, REGISTRY_ID)],
        ),
        (
            "a rule with no filters at all, which covers every repository",
            "unfiltered",
            json!([{"destinations": [destination(REGISTRY_ID, FIRST_REGION)]}]),
            vec![(FIRST_REGION, REGISTRY_ID)],
        ),
        (
            "a prefix filter that does not cover the repository",
            "not-replicated",
            json!([{
                "destinations": [destination(REGISTRY_ID, FIRST_REGION)],
                "repositoryFilters": [prefix("other-")],
            }]),
            vec![],
        ),
        (
            "two destinations in one rule, one of them another account's",
            "two-destinations",
            json!([{
                "destinations": [
                    destination(REGISTRY_ID, FIRST_REGION),
                    destination(FOREIGN_REGISTRY_ID, SECOND_REGION),
                ],
                "repositoryFilters": [prefix("two-")],
            }]),
            vec![(SECOND_REGION, FOREIGN_REGISTRY_ID), (FIRST_REGION, REGISTRY_ID)],
        ),
        (
            "two rules naming one destination, which is one copy of the image",
            "two-rules",
            json!([
                {
                    "destinations": [destination(REGISTRY_ID, FIRST_REGION)],
                    "repositoryFilters": [prefix("two-rules")],
                },
                {
                    "destinations": [destination(REGISTRY_ID, FIRST_REGION)],
                    "repositoryFilters": [prefix("two-")],
                },
            ]),
            vec![(FIRST_REGION, REGISTRY_ID)],
        ),
        (
            "a second filter of the same rule covering the repository",
            "second-filter",
            json!([{
                "destinations": [destination(REGISTRY_ID, SECOND_REGION)],
                "repositoryFilters": [prefix("other-"), prefix("second-")],
            }]),
            vec![(SECOND_REGION, REGISTRY_ID)],
        ),
        (
            "no configuration at all",
            "unconfigured",
            json!([]),
            vec![],
        ),
    ];

    for (case, repository, rules, expected) in rows {
        let router = emulator();
        given_configuration(&router, case, rules).await;
        let digest = given_pushed_image(&router, case, repository).await;

        // Straight after the push, every copy the clock owes is still being made.
        let running = statuses(&router, case, repository, &digest).await;
        let in_progress: Vec<(String, String, String)> = expected
            .iter()
            .map(|(region, registry)| {
                (
                    (*region).to_string(),
                    (*registry).to_string(),
                    state::REPLICATION_STATUS_IN_PROGRESS.to_string(),
                )
            })
            .collect();
        let mut in_progress = in_progress;
        in_progress.sort();
        assert_eq!(
            running, in_progress,
            "case {case:?}: expected {in_progress:?} straight after the push, got {running:?}"
        );

        tokio::time::sleep(PAST_THE_DELAY).await;

        let settled = statuses(&router, case, repository, &digest).await;
        let mut complete: Vec<(String, String, String)> = expected
            .iter()
            .map(|(region, registry)| {
                (
                    (*region).to_string(),
                    (*registry).to_string(),
                    state::REPLICATION_STATUS_COMPLETE.to_string(),
                )
            })
            .collect();
        complete.sort();
        assert_eq!(
            settled, complete,
            "case {case:?}: expected {complete:?} past the owed completion, got {settled:?}"
        );

        // A read reports the same thing twice: the statuses are derived from the stored
        // record and the due instant, so reading one changes nothing.
        let again = statuses(&router, case, repository, &digest).await;
        assert_eq!(
            again, settled,
            "case {case:?}: a second read must report the same statuses, got {again:?}"
        );
    }
}

/// The records one push writes are bounded: a configuration naming exactly the bound's
/// worth of destinations is recorded whole, and one naming more is recorded up to it.
///
/// The destinations are made distinct by their registry id, which is the caller-chosen half
/// of a destination, so the row at the bound and the row past it differ only in how many
/// distinct destinations the configuration names.
#[tokio::test]
async fn the_replications_one_push_records_are_bounded() {
    /// The modelled ceiling on one rule's destinations (`ReplicationDestinationList` is
    /// `@length(min: 0, max: 100)`), which is how a configuration reaches the store's own
    /// bound.
    const MAX_DESTINATIONS_PER_RULE: usize = 100;

    // (case, how many distinct destinations the configuration names, how many are recorded)
    for (case, named, recorded) in [
        (
            "at the bound",
            state::MAX_IMAGE_REPLICATIONS,
            state::MAX_IMAGE_REPLICATIONS,
        ),
        (
            "one destination past the bound",
            state::MAX_IMAGE_REPLICATIONS + 1,
            state::MAX_IMAGE_REPLICATIONS,
        ),
    ] {
        let router = emulator();
        // Each destination is one Region with its own account number: a registry id is
        // `^[0-9]{12}$`, so the index is zero-padded to twelve digits.
        let destinations: Vec<Value> = (0..named)
            .map(|index| destination(&format!("{:012}", index + 1), FIRST_REGION))
            .collect();
        let rules: Vec<Value> = destinations
            .chunks(MAX_DESTINATIONS_PER_RULE)
            .map(|chunk| json!({"destinations": chunk.to_vec()}))
            .collect();
        given_configuration(&router, case, json!(rules)).await;

        let repository = "bounded";
        let digest = given_pushed_image(&router, case, repository).await;
        let reported = statuses(&router, case, repository, &digest).await;
        assert_eq!(
            reported.len(),
            recorded,
            "case {case:?}: expected {recorded} of {named} destinations recorded, got {}",
            reported.len()
        );
    }
}

/// Replication is decided at the push: a rule stored after an image was pushed copies
/// nothing of it, while an image pushed after the rule is copied — and removing the
/// configuration does not withdraw the status of a copy already made.
#[tokio::test]
async fn a_rule_stored_after_a_push_does_not_replicate_what_is_already_held() {
    let router = emulator();
    let repository = "retroactive";
    let case = "pushed before the rule";
    let before = given_pushed_image(&router, case, repository).await;

    given_configuration(
        &router,
        case,
        json!([{
            "destinations": [destination(REGISTRY_ID, FIRST_REGION)],
            "repositoryFilters": [prefix("retro")],
        }]),
    )
    .await;
    tokio::time::sleep(PAST_THE_DELAY).await;
    assert_eq!(
        statuses(&router, case, repository, &before).await,
        Vec::new(),
        "case {case:?}: an image the registry already held is not copied by a later rule"
    );

    // An image pushed after the rule is copied, and the copy settles.
    let content = b"pushed-after-the-rule".to_vec();
    let layer = given_layer(&router, "pushed after the rule", repository, &content).await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": layer,
            "size": content.len(),
        }],
    })
    .to_string();
    ok(
        &router,
        "pushed after the rule",
        "PutImage",
        &json!({"repositoryName": repository, "imageManifest": manifest}),
    )
    .await;
    let after = state::manifest_digest(&manifest);
    tokio::time::sleep(PAST_THE_DELAY).await;
    assert_eq!(
        statuses(&router, "pushed after the rule", repository, &after).await,
        vec![(
            FIRST_REGION.to_string(),
            REGISTRY_ID.to_string(),
            state::REPLICATION_STATUS_COMPLETE.to_string(),
        )],
        "case \"pushed after the rule\": the copy the rule asked for is reported complete"
    );

    // Turning replication off is how a put with no rules reads, and it does not unmake a
    // copy that was already made.
    given_configuration(&router, "configuration removed", json!([])).await;
    assert_eq!(
        statuses(&router, "configuration removed", repository, &after).await,
        vec![(
            FIRST_REGION.to_string(),
            REGISTRY_ID.to_string(),
            state::REPLICATION_STATUS_COMPLETE.to_string(),
        )],
        "case \"configuration removed\": a copy already made keeps its status"
    );
}
