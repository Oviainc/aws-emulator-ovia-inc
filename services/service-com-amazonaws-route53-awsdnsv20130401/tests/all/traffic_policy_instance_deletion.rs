// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteTrafficPolicyInstance`.
//!
//! The rule under test: the operation removes the traffic policy instance the
//! `Id` names AND every resource record set the service created for that
//! instance, so afterwards the account no longer holds the instance, the hosted
//! zone no longer holds its record sets, and the `(hosted zone, DNS name,
//! record type)` the instance occupied is free for a new instance — while every
//! other instance and every record set owned by someone else is left exactly as
//! it was. An `Id` the account holds no instance for is the modelled
//! `NoSuchTrafficPolicyInstance` (HTTP 404) and deletes nothing.
//!
//! So the tables below are not one recorded request: they span the record type
//! the instance generates, its `TTL`, the depth and spelling of its DNS name,
//! the hosted zone it lives in, and whether it is still being created or
//! already applied when the delete arrives — the delete is about the instance's
//! identity, not about any one of those.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other traffic-policy suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::R_R_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::TRAFFIC_POLICY_INSTANCE_PROPAGATION;

use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::traffic_policies::{create_policy, document_with_start_endpoint, policy_of};
use crate::traffic_policy_instances::{create_instance, get_instance, instance_count};
use crate::traffic_policy_not_found_envelope::assert_identity_only;

/// How much longer than [`TRAFFIC_POLICY_INSTANCE_PROPAGATION`] a row waits
/// before it deletes an instance it wants to find already applied, so the case
/// does not race the span itself.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);

/// The published maximum of `TrafficPolicyInstanceId`
/// (`len 1..36`, smithy_query(query="shape",shape="TrafficPolicyInstanceId")).
const INSTANCE_ID_MAX_LEN: usize = 36;

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// Delete one traffic policy instance.
///
/// `pub(crate)` so the traffic-policy DELETION suite clears the instance that
/// blocks a version's delete through the SAME request rather than growing a
/// second copy of it.
pub(crate) async fn delete_instance(router: &MultiServiceRouter, id: &str) -> Reply {
    call(
        router,
        "DELETE",
        &format!("/2013-04-01/trafficpolicyinstance/{id}"),
        "",
    )
    .await
}

/// List a hosted zone's record sets.
async fn list_records(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    let reply = call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from ListResourceRecordSets, got {} with body {}",
        reply.status, reply.body
    );
    reply
}

/// Create a policy whose single version generates `rr_type` record sets, and
/// return its id.
async fn policy_id(router: &MultiServiceRouter, name: &str, rr_type: &str) -> String {
    let document = document_with_start_endpoint(rr_type, "192.0.2.1");
    let created = create_policy(router, name, &document, None).await;
    assert_eq!(
        created.status, 201,
        "creating the policy {name:?}: expected 201, got {} with body {}",
        created.status, created.body
    );
    policy_of(&created, name).id
}

/// Create an instance and return its `Id`, failing the case when the create was
/// refused.
async fn create_instance_ok(
    router: &MultiServiceRouter,
    case: &str,
    zone: &str,
    name: &str,
    ttl: i64,
    policy: &str,
) -> String {
    let created = create_instance(router, zone, name, ttl, policy, 1).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected 201 from CreateTrafficPolicyInstance, got {} with body {}",
        created.status, created.body
    );
    created
        .first("TrafficPolicyInstance/Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create must report TrafficPolicyInstance/Id; body {}",
                created.body
            )
        })
        .to_string()
}

/// How many record sets of `zone` name `id` as the instance they were created
/// for (`ResourceRecordSet$TrafficPolicyInstanceId`).
async fn records_owned_by(router: &MultiServiceRouter, zone: &str, id: &str) -> usize {
    list_records(router, zone)
        .await
        .all("ResourceRecordSet/TrafficPolicyInstanceId")
        .iter()
        .filter(|owner| **owner == id)
        .count()
}

/// How many record sets `zone` holds in total.
async fn record_count(router: &MultiServiceRouter, zone: &str) -> usize {
    list_records(router, zone)
        .await
        .all("ResourceRecordSet/Name")
        .len()
}

// ---------------------------------------------------------------------------
// The delete
// ---------------------------------------------------------------------------

/// One row of the delete table: an instance to create, and when to delete it.
struct DeleteRow {
    case: &'static str,
    /// The `RRType` the row's policy version generates, which is the type of
    /// the record set the delete has to take away.
    rr_type: &'static str,
    /// The `Name` as the request spells it.
    name: &'static str,
    ttl: i64,
    /// Whether the row waits for the instance's record sets to be in place
    /// before deleting it, so the table spans both an instance still being
    /// created and one already applied.
    settle_first: bool,
    /// Which of the two hosted zones the row's instance goes in.
    zone: usize,
}

#[tokio::test]
async fn deleting_an_instance_removes_it_and_the_record_sets_it_created() {
    let rows = [
        DeleteRow {
            case: "an A instance at a subdomain, deleted while it is still being created",
            rr_type: "A",
            name: "www.example.com",
            ttl: 60,
            settle_first: false,
            zone: 0,
        },
        DeleteRow {
            case: "an AAAA instance deeper in the zone, deleted once it is applied",
            rr_type: "AAAA",
            name: "a.b.example.com",
            ttl: 0,
            settle_first: true,
            zone: 0,
        },
        DeleteRow {
            case: "a TXT instance at the zone apex, name written with the root dot",
            rr_type: "TXT",
            name: "example.com.",
            ttl: 86_400,
            settle_first: false,
            zone: 0,
        },
        DeleteRow {
            case: "a CNAME instance in the other hosted zone, name spelled in mixed case",
            rr_type: "CNAME",
            name: "Shop.Example.NET",
            ttl: 300,
            settle_first: false,
            zone: 1,
        },
    ];

    // Every type the table spans is one the model's own `RRType` enum names, so
    // no row deletes a record set a traffic policy instance could not have
    // generated.
    for row in &rows {
        assert!(
            R_R_TYPE_VALUES.contains(&row.rr_type),
            "case {:?}: {} is not a modelled RRType",
            row.case,
            row.rr_type
        );
    }

    let router = router();
    let zones = [
        create_zone_ok(&router, "example.com", "instance-deletes-a").await,
        create_zone_ok(&router, "example.net", "instance-deletes-b").await,
    ];

    // A bystander instance, in the FIRST zone, that no row deletes: its record
    // set and its own read have to survive every delete below.
    let keeper_policy = policy_id(&router, "keeper-policy", "A").await;
    let keeper = create_instance_ok(
        &router,
        "the bystander instance",
        &zones[0],
        "keep.example.com",
        120,
        &keeper_policy,
    )
    .await;

    // The record sets each zone holds with only the bystander in place: the
    // zone's own SOA and NS records, plus the bystander's one record set in the
    // first zone.
    let mut base: Vec<usize> = Vec::new();
    for zone in &zones {
        base.push(record_count(&router, zone).await);
    }

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let zone = &zones[row.zone];
        let policy = policy_id(&router, &format!("delete-policy-{index}"), row.rr_type).await;
        let id = create_instance_ok(&router, case, zone, row.name, row.ttl, &policy).await;

        // The create put the instance and its record set in place: that is what
        // the delete has to take away.
        assert_eq!(
            records_owned_by(&router, zone, &id).await,
            1,
            "case {case:?}: the create must leave one record set naming the instance {id:?}"
        );
        assert_eq!(
            record_count(&router, zone).await,
            base[row.zone] + 1,
            "case {case:?}: the create must add exactly one record set to the zone"
        );

        if row.settle_first {
            tokio::time::sleep(TRAFFIC_POLICY_INSTANCE_PROPAGATION + SETTLE_MARGIN).await;
        }

        let deleted = delete_instance(&router, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from DeleteTrafficPolicyInstance, got {} with body {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.error_code().is_empty(),
            "case {case:?}: a served delete must carry no error, got {:?}; body {}",
            deleted.error_code(),
            deleted.body
        );

        // The instance is gone: the read that reported it now reports the
        // modelled refusal, by its code alone.
        let read = get_instance(&router, &id).await;
        assert_identity_only(&read, case, "NoSuchTrafficPolicyInstance");

        // A second delete of the same id has nothing left to delete.
        let again = delete_instance(&router, &id).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: expected 404 from a second delete, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "NoSuchTrafficPolicyInstance",
            "case {case:?}: a second delete must answer NoSuchTrafficPolicyInstance; body {}",
            again.body
        );

        // The record sets the instance owned are gone with it, and the zone is
        // back to what it held before the create.
        assert_eq!(
            records_owned_by(&router, zone, &id).await,
            0,
            "case {case:?}: the zone must hold no record set naming the deleted instance {id:?}"
        );
        assert_eq!(
            record_count(&router, zone).await,
            base[row.zone],
            "case {case:?}: the delete must take away exactly the record sets it created"
        );

        // The bystander is untouched: its record set is still listed and its
        // own read still reports it.
        assert_eq!(
            records_owned_by(&router, &zones[0], &keeper).await,
            1,
            "case {case:?}: the record set of another instance must survive the delete"
        );
        let kept = get_instance(&router, &keeper).await;
        assert_eq!(
            kept.status, 200,
            "case {case:?}: the other instance must still read, got {} with body {}",
            kept.status, kept.body
        );

        // The account holds only the bystander again.
        assert_eq!(
            instance_count(&router).await,
            "1",
            "case {case:?}: the account must hold exactly the instances that were not deleted"
        );

        // The identity the deleted instance occupied is FREE: a new instance
        // over the same zone, name and type is served rather than refused as
        // already existing. Deleted again, so the next row starts from the same
        // population this one did.
        let reused = create_instance(&router, zone, row.name, row.ttl, &policy, 1).await;
        assert_eq!(
            reused.status, 201,
            "case {case:?}: the deleted instance must free its name and type, got {} with \
             body {}",
            reused.status, reused.body
        );
        let reused_id = reused
            .first("TrafficPolicyInstance/Id")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the re-create must report an Id; body {}",
                    reused.body
                )
            })
            .to_string();
        assert_ne!(
            reused_id, id,
            "case {case:?}: the re-created instance must be a new instance, not the deleted id"
        );
        let cleaned = delete_instance(&router, &reused_id).await;
        assert_eq!(
            cleaned.status, 200,
            "case {case:?}: expected 200 deleting the re-created instance, got {} with body {}",
            cleaned.status, cleaned.body
        );
    }

    // After the whole walk only the bystander is left, in both the account's
    // count and the zones' record sets.
    assert_eq!(
        instance_count(&router).await,
        "1",
        "only the instance no row deleted may be left"
    );
    for (index, zone) in zones.iter().enumerate() {
        assert_eq!(
            record_count(&router, zone).await,
            base[index],
            "zone {zone:?} must hold exactly the record sets no delete took away"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_naming_no_instance_is_refused_and_deletes_nothing() {
    let router = router();
    let zone = create_zone_ok(&router, "example.com", "instance-delete-refusals").await;
    let policy = policy_id(&router, "refusal-policy", "A").await;
    let held = create_instance_ok(
        &router,
        "the held instance",
        &zone,
        "www.example.com",
        60,
        &policy,
    )
    .await;

    // One row per way an `Id` can name no instance the account holds: a
    // well-formed id nobody minted, the id of another kind of resource, and an
    // id of no minted form at all.
    let rows = [
        (
            "an unminted id of the right form",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("a traffic policy's id", policy.clone()),
        ("a hosted zone's id", zone.clone()),
        ("an id of no recognised form", "not-an-instance".to_string()),
        ("an id one character long", "x".to_string()),
    ];

    for (case, id) in &rows {
        let reply = delete_instance(&router, id).await;
        // A traffic policy instance the account does not hold is reported by its
        // code alone: `Type` and `Code`, with no `Message` element.
        assert_identity_only(&reply, case, "NoSuchTrafficPolicyInstance");
    }

    // One past the published bound of `TrafficPolicyInstanceId`: the model
    // refuses it as a bad request before any lookup, so it is never reported as
    // an instance that does not exist.
    let too_long = "0".repeat(INSTANCE_ID_MAX_LEN + 1);
    let refused = delete_instance(&router, &too_long).await;
    assert_eq!(
        refused.status, 400,
        "an id past the modelled length bound must be refused as a bad request, got {} with \
         body {}",
        refused.status, refused.body
    );
    assert_ne!(
        refused.error_code(),
        "NoSuchTrafficPolicyInstance",
        "an id the model refuses must not be reported as a missing instance; body {}",
        refused.body
    );

    // Nothing any of the refused deletes named was deleted: the instance the
    // account does hold still reads, still owns its record set, and is still
    // counted.
    let read = get_instance(&router, &held).await;
    assert_eq!(
        read.status, 200,
        "the held instance must still read, got {} with body {}",
        read.status, read.body
    );
    assert_eq!(
        records_owned_by(&router, &zone, &held).await,
        1,
        "a refused delete must leave the held instance's record set in place"
    );
    assert_eq!(
        instance_count(&router).await,
        "1",
        "a refused delete must not change how many instances the account holds"
    );
}
