// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the `HealthCheckId` a record set may carry.
//!
//! The rule under test: a record set's `HealthCheckId` is a REFERENCE to a health check the
//! account holds, so a change that WRITES one (`CREATE`, `UPSERT`) resolves it against the
//! health checks that exist and is refused with `NoSuchHealthCheck` (HTTP 404) only for an id
//! that names none — the change is otherwise applied, 200, and the record set keeps the
//! reference every later read reports. The reference is resolved once, where it is written:
//! "Amazon Route 53 does not prevent you from deleting a health check even if the health check
//! is associated with one or more resource record sets", so deleting the referenced health
//! check leaves the record set holding a now-dangling reference rather than changing or
//! removing it — and a `DELETE`, which names the values of a record set that exists in order
//! to remove it, withdraws such a reference rather than making one, so that record set is
//! still deletable.
//!
//! The rows differ along what the rule generalises over: the KIND of health check referenced
//! (an endpoint probe on an address and on a name, a TCP probe, a CloudWatch-alarm reader and
//! a calculated check over a child), the routing policy of the referring record set (weighted,
//! failover, latency, multivalue answer and simple), the record type and how many values it
//! carries, the change action (`CREATE` and `UPSERT`), and how many references one batch
//! carries.
//!
//! The refusal half varies the FORM of the id that resolves to nothing — one of the shape
//! this service mints, one in another service's shape, a bare word, and the id of a health
//! check that was deleted — and checks that such a batch is applied in no part, which is what
//! "change batches are considered transactional changes" requires.
//!
//! Every test drives the service through the real router, using the same harness the other
//! record-set and health-check suites use.

use smithy_server_runtime::MultiServiceRouter;

use crate::health_check_deletion::delete_health_check;
use crate::health_checks::{create_health_check, health_check_id};
use crate::hosted_zone_records::{call, change_batch_body, create_zone_ok, router, Reply};

/// The zone every row writes its record sets in.
const ZONE: &str = "health-check-refs.example.";

/// One row: a health check to create, and the record set that references it.
struct Row {
    case: &'static str,
    /// The `HealthCheckConfig` the referenced health check is created with. A `CALCULATED`
    /// row names its child by the `{child}` placeholder.
    config: &'static str,
    /// The record set's own members, beside the `HealthCheckId` the test fills in: its name,
    /// type, TTL, values and the routing members that make its policy.
    name: &'static str,
    rr_type: &'static str,
    ttl: &'static str,
    values: &'static [&'static str],
    routing: &'static str,
    /// The action the batch applies the record set with.
    action: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "an HTTP check on an address referenced by a weighted A record",
        config: "<Type>HTTP</Type><IPAddress>192.0.2.47</IPAddress><Port>80</Port>\
                 <ResourcePath>/</ResourcePath><RequestInterval>30</RequestInterval>\
                 <FailureThreshold>3</FailureThreshold>",
        name: "weighted.health-check-refs.example.",
        rr_type: "A",
        ttl: "60",
        values: &["192.0.2.47"],
        routing: "<SetIdentifier>primary</SetIdentifier><Weight>100</Weight>",
        action: "CREATE",
    },
    Row {
        case: "an HTTPS check on a domain name referenced by a failover A record",
        config: "<Type>HTTPS</Type><FullyQualifiedDomainName>api.health-check-refs.example\
                 </FullyQualifiedDomainName><Port>443</Port>\
                 <ResourcePath>/prod/healthy</ResourcePath>",
        name: "failover.health-check-refs.example.",
        rr_type: "A",
        ttl: "30",
        values: &["192.0.2.48", "192.0.2.49"],
        routing: "<SetIdentifier>east</SetIdentifier><Failover>PRIMARY</Failover>",
        action: "CREATE",
    },
    Row {
        case: "a TCP check referenced by a latency AAAA record",
        config: "<Type>TCP</Type><IPAddress>192.0.2.50</IPAddress><Port>8080</Port>",
        name: "latency.health-check-refs.example.",
        rr_type: "AAAA",
        ttl: "120",
        values: &["2001:db8::1"],
        routing: "<SetIdentifier>us-west-2</SetIdentifier><Region>us-west-2</Region>",
        action: "UPSERT",
    },
    Row {
        case: "a CloudWatch-alarm check referenced by a multivalue A record",
        config: "<Type>CLOUDWATCH_METRIC</Type><AlarmIdentifier><Region>us-east-1</Region>\
                 <Name>endpoint-unhealthy</Name></AlarmIdentifier>",
        name: "multivalue.health-check-refs.example.",
        rr_type: "A",
        ttl: "15",
        values: &["192.0.2.51"],
        routing: "<SetIdentifier>one</SetIdentifier>\
                  <MultiValueAnswer>true</MultiValueAnswer>",
        action: "CREATE",
    },
    Row {
        case: "a CALCULATED check over one child referenced by a simple CNAME record",
        config: "<Type>CALCULATED</Type><HealthThreshold>1</HealthThreshold>\
                 <ChildHealthChecks><ChildHealthCheck>{child}</ChildHealthCheck>\
                 </ChildHealthChecks>",
        name: "simple.health-check-refs.example.",
        rr_type: "CNAME",
        ttl: "300",
        values: &["origin.health-check-refs.example."],
        routing: "",
        action: "UPSERT",
    },
];

/// The `<Change>` element one row's record set is applied with under `action`, carrying
/// `health_check_id` as its reference.
fn change_of(row: &Row, action: &str, health_check_id: &str) -> String {
    let records: String = row
        .values
        .iter()
        .map(|value| format!("<ResourceRecord><Value>{value}</Value></ResourceRecord>"))
        .collect();
    format!(
        "<Change><Action>{action}</Action><ResourceRecordSet><Name>{}</Name><Type>{}</Type>{}\
         <TTL>{}</TTL><ResourceRecords>{records}</ResourceRecords>\
         <HealthCheckId>{health_check_id}</HealthCheckId></ResourceRecordSet></Change>",
        row.name, row.rr_type, row.routing, row.ttl
    )
}

/// Submit one batch of already-rendered `<Change>` elements against `zone_id`.
async fn submit(router: &MultiServiceRouter, zone_id: &str, changes: &str) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        &change_batch_body(changes, None),
    )
    .await
}

/// The record set `zone_id` holds at `name` and `rr_type`, read back through
/// `ListResourceRecordSets` from that very position.
async fn read_back(
    router: &MultiServiceRouter,
    zone_id: &str,
    name: &str,
    rr_type: &str,
) -> Reply {
    call(
        router,
        "GET",
        &format!(
            "/2013-04-01/hostedzone/{zone_id}/rrset?name={name}&type={rr_type}&maxitems=1"
        ),
        "",
    )
    .await
}

/// Create the health check one row references and return its id.
async fn referenced_check(
    router: &MultiServiceRouter,
    row: &Row,
    reference: &str,
    child: &str,
) -> String {
    let config = row.config.replace("{child}", child);
    let created = create_health_check(router, reference, &config).await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected 201 from CreateHealthCheck, got {} with body {}",
        row.case, created.status, created.body
    );
    health_check_id(&created, row.case)
}

#[tokio::test]
async fn a_record_set_may_reference_a_health_check_the_account_holds() {
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "health-check-refs").await;
    // The child a CALCULATED row names, which is also a health check no row deletes.
    let child = create_health_check(
        &router,
        "health-check-refs-child",
        "<Type>TCP</Type><IPAddress>192.0.2.60</IPAddress><Port>443</Port>",
    )
    .await;
    let child_id = health_check_id(&child, "the child health check");

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let id = referenced_check(
            &router,
            row,
            &format!("health-check-refs-{index}"),
            &child_id,
        )
        .await;

        // The change is ACCEPTED: the referenced health check exists.
        let applied = submit(&router, &zone_id, &change_of(row, row.action, &id)).await;
        assert_eq!(
            applied.status, 200,
            "case {case:?}: expected HTTP 200 from ChangeResourceRecordSets for a record set \
             referencing an existing health check, got {} with body {}",
            applied.status, applied.body
        );
        assert_eq!(
            applied.first("ChangeInfo/Status"),
            Some("PENDING"),
            "case {case:?}: expected the change to be reported PENDING, got body {}",
            applied.body
        );

        // The reference is PERSISTED: the record set reports the id it was written with.
        let listed = read_back(&router, &zone_id, row.name, row.rr_type).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListResourceRecordSets, got {} with body {}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.first("ResourceRecordSet/Name"),
            Some(row.name),
            "case {case:?}: expected the zone to hold {:?}, got body {}",
            row.name,
            listed.body
        );
        assert_eq!(
            listed.first("ResourceRecordSet/HealthCheckId"),
            Some(id.as_str()),
            "case {case:?}: expected the record set to report HealthCheckId {id:?}, got body {}",
            listed.body
        );

        // Deleting the referenced health check is permitted and leaves the record set
        // holding the reference it was written with.
        let deleted = delete_health_check(&router, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteHealthCheck on a referenced health \
             check, got {} with body {}",
            deleted.status, deleted.body
        );
        let dangling = read_back(&router, &zone_id, row.name, row.rr_type).await;
        assert_eq!(
            dangling.first("ResourceRecordSet/HealthCheckId"),
            Some(id.as_str()),
            "case {case:?}: expected the record set to keep the dangling HealthCheckId {id:?} \
             after the health check was deleted, got body {}",
            dangling.body
        );
        assert_eq!(
            dangling.all("ResourceRecord/Value"),
            row.values,
            "case {case:?}: expected the record set's values to be unchanged by the delete, \
             got body {}",
            dangling.body
        );

        // And that record set is still DELETABLE: a `DELETE` names the values of a record
        // set that exists in order to remove it, so it withdraws the reference rather than
        // making one, and a dangling id cannot strand the record in the zone.
        let removed = submit(&router, &zone_id, &change_of(row, "DELETE", &id)).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected HTTP 200 from a DELETE of the record set carrying the \
             dangling HealthCheckId {id:?}, got {} with body {}",
            removed.status, removed.body
        );
        let gone = read_back(&router, &zone_id, row.name, row.rr_type).await;
        assert_ne!(
            gone.first("ResourceRecordSet/Name"),
            Some(row.name),
            "case {case:?}: expected the record set to be gone after the DELETE, got body {}",
            gone.body
        );
    }
}

#[tokio::test]
async fn one_batch_may_reference_several_health_checks() {
    // The rule holds per REFERENCE, not per batch: a batch carrying two record sets that
    // name two different existing health checks is applied whole.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "health-check-refs-batch").await;
    let first = create_health_check(
        &router,
        "batch-first",
        "<Type>HTTP</Type><IPAddress>192.0.2.70</IPAddress><Port>80</Port>",
    )
    .await;
    let first_id = health_check_id(&first, "the first health check");
    let second = create_health_check(
        &router,
        "batch-second",
        "<Type>HTTPS</Type><FullyQualifiedDomainName>second.health-check-refs.example\
         </FullyQualifiedDomainName><Port>443</Port>",
    )
    .await;
    let second_id = health_check_id(&second, "the second health check");

    let changes = format!(
        "{}{}",
        change_of(&ROWS[0], ROWS[0].action, &first_id),
        change_of(&ROWS[1], ROWS[1].action, &second_id)
    );
    let applied = submit(&router, &zone_id, &changes).await;
    assert_eq!(
        applied.status, 200,
        "expected HTTP 200 for a batch referencing two existing health checks, got {} with \
         body {}",
        applied.status, applied.body
    );
    for (row, id) in [(&ROWS[0], &first_id), (&ROWS[1], &second_id)] {
        let listed = read_back(&router, &zone_id, row.name, row.rr_type).await;
        assert_eq!(
            listed.first("ResourceRecordSet/HealthCheckId"),
            Some(id.as_str()),
            "case {:?}: expected the record set to report HealthCheckId {id:?}, got body {}",
            row.case,
            listed.body
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_health_check_refuses_the_whole_batch() {
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "health-check-refs-missing").await;
    // A health check created and then DELETED: its id named a health check once and names
    // none now, which is exactly the state a new reference to it must be refused in.
    let gone = create_health_check(
        &router,
        "missing-deleted",
        "<Type>HTTP</Type><IPAddress>192.0.2.80</IPAddress><Port>80</Port>",
    )
    .await;
    let gone_id = health_check_id(&gone, "the deleted health check");
    let deleted = delete_health_check(&router, &gone_id).await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeleteHealthCheck, got {} with body {}",
        deleted.status, deleted.body
    );
    // One health check that DOES exist, so a batch refused below is refused over the
    // missing reference alone.
    let held = create_health_check(
        &router,
        "missing-held",
        "<Type>TCP</Type><IPAddress>192.0.2.81</IPAddress><Port>443</Port>",
    )
    .await;
    let held_id = health_check_id(&held, "the held health check");

    // The ids differ along form and length: one of the shape this service mints, one in
    // another service's form, a bare word, and the id of a deleted health check.
    for (case, missing) in [
        ("an unminted id of the form this service mints", "01234567-89ab-cdef-0123-456789abcdef".to_string()),
        ("an id in another service's form", "Z1D633PJN98FT9".to_string()),
        ("a bare word", "not-an-id".to_string()),
        ("the id of a deleted health check", gone_id.clone()),
    ] {
        let refused =
            submit(&router, &zone_id, &change_of(&ROWS[0], ROWS[0].action, &missing)).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for a HealthCheckId that names no health \
             check, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "NoSuchHealthCheck",
            "case {case:?}: expected NoSuchHealthCheck, got {:?}; body {}",
            refused.error_code(),
            refused.body
        );
        let message = refused.first("Error/Message").unwrap_or("");
        assert!(
            message.contains(&missing),
            "case {case:?}: the refusal must name the id asked about ({missing:?}), got \
             {message:?}"
        );

        // The batch was applied in no part: the record set it carried is not in the zone.
        let listed = read_back(&router, &zone_id, ROWS[0].name, ROWS[0].rr_type).await;
        assert_ne!(
            listed.first("ResourceRecordSet/Name"),
            Some(ROWS[0].name),
            "case {case:?}: the refused batch must create no record set, got body {}",
            listed.body
        );

        // And a batch whose OTHER change is well-formed is refused with it, since "change
        // batches are considered transactional changes".
        let changes = format!(
            "{}{}",
            change_of(&ROWS[1], ROWS[1].action, &held_id),
            change_of(&ROWS[0], ROWS[0].action, &missing)
        );
        let refused = submit(&router, &zone_id, &changes).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for a batch one of whose references names no \
             health check, got {} with body {}",
            refused.status, refused.body
        );
        let listed = read_back(&router, &zone_id, ROWS[1].name, ROWS[1].rr_type).await;
        assert_ne!(
            listed.first("ResourceRecordSet/Name"),
            Some(ROWS[1].name),
            "case {case:?}: the refused batch must leave its other change unapplied, got \
             body {}",
            listed.body
        );
    }

    // The reference that DOES resolve is accepted against the same zone, which is what
    // makes the refusals above about the missing ids rather than about the batch's shape.
    let applied =
        submit(&router, &zone_id, &change_of(&ROWS[1], ROWS[1].action, &held_id)).await;
    assert_eq!(
        applied.status, 200,
        "expected HTTP 200 for the reference that resolves, got {} with body {}",
        applied.status, applied.body
    );
}
