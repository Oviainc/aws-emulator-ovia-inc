// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-INSTANCE restriction on the Region WRITES, end to end over the `awsJson1_1`
//! wire.
//!
//! THE RULE: replicating an IAM Identity Center instance across Regions is a feature of the
//! ORGANIZATION instance, so on an ACCOUNT instance — the instance of a single standalone
//! account, which is what `CreateInstance` creates and what an account discovers through
//! `ListInstances` — every operation that MUTATES the instance's set of Regions
//! (`AddRegion`, `RemoveRegion`) is refused with the modelled `ValidationException`
//! (`smithy_query(query="operation",operation="AddRegion,RemoveRegion")` lists it,
//! `errorKind` client, `httpError` 400) carrying
//! `state::ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE`, and the refusal is answered BEFORE
//! anything about the Region the request names is judged — before "already enabled", before
//! "the primary Region cannot be removed", before "the request must be made from the
//! primary Region" and before the Region is looked for at all, because on such an instance
//! no Region can ever be added or removed. The REPORTING side of the same surface
//! (`ListRegions`, `DescribeRegion`) is served: an account instance is enabled in its own
//! primary Region and reports it. An `InstanceArn` the account holds no instance under is
//! still that operation's own missing-resource answer, because the restriction belongs to a
//! RESOLVED instance; and on an organization instance the same writes are served, which is
//! what makes this a property of the instance KIND rather than of the operations.
//!
//! The table spans what the rule generalises over: both writes, the Region names the
//! model's `RegionName` pattern `^([a-z]+-){2,3}\d$` admits (two- and three-prefix
//! spellings, several geographies, GovCloud and China), and the RELATION the named Region
//! has to the instance — its own primary Region, an additional Region it is already enabled
//! in, one with a workflow in flight, and one it was never enabled in — none of which can
//! change the answer. The recorded case (an `AddRegion` and a `RemoveRegion` naming the
//! instance's primary Region) is two rows of it.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns; the account
//! instance is reached the way a client reaches one, through `CreateInstance`, and the
//! Region records a row needs beside the primary one are seeded through the service's own
//! `state::Instance::add_region` (the add over the wire is `instance_region_addition.rs`,
//! which drives it on the organization instance that serves it).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::{empty_account_router, hold_activation};
use crate::ssoadmin_permission_sets::{
    call, call_in_region, error_code, organization_router, with_store, TEST_REGION,
    UNKNOWN_INSTANCE_ARN,
};

/// The Region the account instance of every fixture below is enabled in: the Region its
/// `CreateInstance` was signed for, which is what `PrimaryRegion` reports.
const PRIMARY_REGION: &str = TEST_REGION;

/// An ADDITIONAL Region the account instance is seeded as enabled in, so a row can name a
/// Region that really is one of the instance's without an `AddRegion` the rule refuses.
const ENABLED_ADDITIONAL: &str = "eu-west-1";

/// A Region of the instance carrying a workflow in flight, so a row can name one that
/// would otherwise be the modelled `ConflictException`.
const BUSY_ADDITIONAL: &str = "ap-south-1";

/// A router over an account that holds an ACTIVE ACCOUNT instance, that engine, and the
/// instance's ARN.
///
/// Built over the WIRE: `CreateInstance` is documented as creating "an instance of IAM
/// Identity Center for a standalone Amazon Web Services account", so the instance it mints
/// is an account instance; its activation instant is then moved into the past the way the
/// clock moves one, so the instance under test is the ACTIVE instance a caller sees once
/// the creation has finished.
async fn account_instance() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String) {
    let (router, engine) = empty_account_router();
    let (status, created) = call(&router, "CreateInstance", &json!({"Name": "Standalone"})).await;
    assert_eq!(
        status, 200,
        "CreateInstance expected 200, got {status} {created}"
    );
    let instance_arn = created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateInstance minted no InstanceArn: {created}"))
        .to_string();
    hold_activation(&engine, SystemTime::now() - Duration::from_secs(60));
    (router, engine, instance_arn)
}

/// The same account instance, additionally holding [`ENABLED_ADDITIONAL`] as an `ACTIVE`
/// Region and [`BUSY_ADDITIONAL`] with its extension workflow still in flight.
///
/// The records go in through `state::Instance::add_region` — the one place this service's
/// own records append a Region — because the operation that would add them over the wire is
/// the very one under test here; everything asserted afterwards goes over the wire.
async fn account_instance_holding_regions(
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String) {
    let (router, engine, instance_arn) = account_instance().await;
    let busy_until = SystemTime::now() + Duration::from_secs(3_600);
    with_store(&engine, |store| {
        let instance = store
            .instance_mut(&instance_arn)
            .expect("the instance the create just stored");
        for name in [ENABLED_ADDITIONAL, BUSY_ADDITIONAL] {
            instance
                .add_region(state::Region::active(
                    name,
                    DateTime::from(SystemTime::now()),
                ))
                .expect("the seeded Regions stay under the per-instance ceiling");
        }
        let busy = instance
            .region_mut(BUSY_ADDITIONAL)
            .expect("the Region just seeded");
        busy.status = "ADDING".to_string();
        busy.active_due = Some(busy_until);
    });
    (router, engine, instance_arn)
}

/// One row of the table: a Region write, the Region it names and the Region the call is
/// made in.
struct Row {
    label: &'static str,
    operation: &'static str,
    region_name: &'static str,
    called_from: &'static str,
}

/// The rows the rule is asserted over.
///
/// They differ along the Region NAME (both spellings the `RegionName` pattern admits,
/// several geographies, the GovCloud and China partitions), along the RELATION that Region
/// has to the instance (its primary one, an additional one it is enabled in, one whose
/// workflow is in flight, one it was never enabled in), along the Region the CALL is made
/// in (the instance's primary Region, which is the only one `RemoveRegion` otherwise
/// accepts, and another one), and over both writes.
const ROWS: &[Row] = &[
    Row {
        label: "AddRegion naming the instance's own primary Region",
        operation: "AddRegion",
        region_name: PRIMARY_REGION,
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "AddRegion naming an additional Region it is already enabled in",
        operation: "AddRegion",
        region_name: ENABLED_ADDITIONAL,
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "AddRegion naming a Region whose workflow is in flight",
        operation: "AddRegion",
        region_name: BUSY_ADDITIONAL,
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "AddRegion naming a Region it was never enabled in",
        operation: "AddRegion",
        region_name: "us-west-2",
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "AddRegion of a three-prefix Region name, called from elsewhere",
        operation: "AddRegion",
        region_name: "ap-southeast-4",
        called_from: "eu-north-1",
    },
    Row {
        label: "AddRegion of a GovCloud Region",
        operation: "AddRegion",
        region_name: "us-gov-west-1",
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "AddRegion of a China-partition Region",
        operation: "AddRegion",
        region_name: "cn-northwest-1",
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "RemoveRegion naming the instance's own primary Region",
        operation: "RemoveRegion",
        region_name: PRIMARY_REGION,
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "RemoveRegion naming an additional Region it is enabled in",
        operation: "RemoveRegion",
        region_name: ENABLED_ADDITIONAL,
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "RemoveRegion naming a Region whose workflow is in flight",
        operation: "RemoveRegion",
        region_name: BUSY_ADDITIONAL,
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "RemoveRegion naming a Region it was never enabled in",
        operation: "RemoveRegion",
        region_name: "us-west-2",
        called_from: PRIMARY_REGION,
    },
    Row {
        label: "RemoveRegion called from a Region that is not the primary one",
        operation: "RemoveRegion",
        region_name: ENABLED_ADDITIONAL,
        called_from: "eu-north-1",
    },
    Row {
        label: "RemoveRegion of a three-prefix Region name",
        operation: "RemoveRegion",
        region_name: "ap-southeast-4",
        called_from: PRIMARY_REGION,
    },
];

/// `DescribeInstance` for `instance_arn`, as a 200 body — what a later caller sees.
async fn describe(router: &MultiServiceRouter, case: &str, instance_arn: &str) -> Value {
    let (status, body) = call(
        router,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstance expected 200, got {status} {body}"
    );
    body
}

/// `ListRegions` for `instance_arn`, as a 200 body.
async fn list_regions(router: &MultiServiceRouter, case: &str, instance_arn: &str) -> Value {
    let (status, body) = call(router, "ListRegions", &json!({"InstanceArn": instance_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListRegions expected 200, got {status} {body}"
    );
    body
}

/// The `Message` an error body carries, whichever member the renderer put it in.
fn message_of(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// THE RULE, over the whole table: every Region write on an account instance is refused
/// 400 `ValidationException` with AWS's account-instance sentence, and changes nothing a
/// later caller sees.
#[tokio::test]
async fn a_region_write_on_an_account_instance_is_refused() {
    let (router, _engine, instance_arn) = account_instance_holding_regions().await;

    for row in ROWS {
        let case = row.label;
        let before = describe(&router, case, &instance_arn).await;
        let regions_before = list_regions(&router, case, &instance_arn).await;

        let (status, refused) = call_in_region(
            &router,
            row.called_from,
            row.operation,
            &json!({"InstanceArn": instance_arn, "RegionName": row.region_name}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            message_of(&refused),
            state::ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE,
            "case {case:?}: expected the account-instance sentence, got {refused}"
        );

        // The refusal is PRE-MUTATION: both readers answer exactly what they answered
        // before, so no Region was enabled, none was put into a removal workflow, and the
        // instance itself is untouched.
        let after = describe(&router, case, &instance_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused Region write must change nothing a later caller \
             sees, got {after} vs {before}"
        );
        let regions_after = list_regions(&router, case, &instance_arn).await;
        assert_eq!(
            regions_after, regions_before,
            "case {case:?}: a refused Region write must leave the Region set alone, got \
             {regions_after} vs {regions_before}"
        );
    }
}

/// The refusal PRECEDES every check on the Region the request names: each row below would
/// earn a DIFFERENT refusal on an instance that serves Region writes — the already-enabled
/// conflict, the workflow conflict, the primary-Region rule, the "called from the primary
/// Region" rule, the missing Region — and on an account instance every one of them answers
/// the account-instance sentence instead.
#[tokio::test]
async fn the_refusal_precedes_every_check_on_the_region() {
    let (router, _engine, instance_arn) = account_instance_holding_regions().await;

    // What each row would be refused with on an instance that serves the write, as the
    // handlers' own later checks word it; none of these may be the answer here.
    let other_refusals = [
        ("already enabled", "is already enabled in Region"),
        ("a workflow in progress", "workflow is already in progress"),
        ("the primary Region", "cannot be removed"),
        ("not the primary Region", "must be called in the primary Region"),
        (
            "a Region the instance is not enabled in",
            "is not enabled in Region",
        ),
    ];

    for row in ROWS {
        let case = row.label;
        let (_, refused) = call_in_region(
            &router,
            row.called_from,
            row.operation,
            &json!({"InstanceArn": instance_arn, "RegionName": row.region_name}),
        )
        .await;
        let message = message_of(&refused);
        assert_eq!(
            message,
            state::ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE,
            "case {case:?}: the instance-kind guard runs first, got {refused}"
        );
        for (what, fragment) in other_refusals {
            assert!(
                !message.contains(fragment),
                "case {case:?}: the answer must not be the {what} refusal, got {message:?}"
            );
        }
    }
}

/// The restriction belongs to a RESOLVED instance, so an `InstanceArn` this account holds
/// no instance under is still each operation's own missing-resource answer: the
/// `AccessDeniedException` (HTTP 403) that is `AddRegion`'s only not-found channel, and the
/// `ResourceNotFoundException` `RemoveRegion` models.
#[tokio::test]
async fn an_unknown_instance_is_still_the_operations_own_not_found() {
    let (router, _engine, held) = account_instance().await;
    assert_ne!(
        held, UNKNOWN_INSTANCE_ARN,
        "the unknown ARN must not be the one this account holds"
    );

    let rows = [
        ("AddRegion", 403u16, "AccessDeniedException"),
        ("RemoveRegion", 400u16, "ResourceNotFoundException"),
    ];
    for (operation, expected_status, expected_code) in rows {
        let (status, body) = call(
            &router,
            operation,
            &json!({"InstanceArn": UNKNOWN_INSTANCE_ARN, "RegionName": "us-west-2"}),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {operation}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {operation}: expected {expected_code}, got {body}"
        );
        assert_ne!(
            message_of(&body),
            state::ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE,
            "case {operation}: an unknown ARN is a missing resource, not the \
             account-instance restriction, got {body}"
        );
    }
}

/// The REPORTING side of the same surface is served on an account instance: it is enabled
/// in its own primary Region, so the reads answer that Region rather than the restriction.
#[tokio::test]
async fn the_region_reads_are_served_on_an_account_instance() {
    let (router, _engine, instance_arn) = account_instance().await;

    let listed = list_regions(&router, "ListRegions", &instance_arn).await;
    assert_eq!(
        listed["Regions"]
            .as_array()
            .map(|regions| regions
                .iter()
                .map(|entry| entry["RegionName"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()),
        Some(vec![PRIMARY_REGION.to_string()]),
        "an account instance lists the Region it was enabled in, got {listed}"
    );

    let (status, described) = call(
        &router,
        "DescribeRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": PRIMARY_REGION}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeRegion expected 200 on an account instance, got {status} {described}"
    );
    assert_eq!(
        (
            described["RegionName"].as_str(),
            described["IsPrimaryRegion"].as_bool()
        ),
        (Some(PRIMARY_REGION), Some(true)),
        "the account instance describes its primary Region, got {described}"
    );
}

/// The same writes on an ORGANIZATION instance: they are SERVED, which is what makes the
/// refusal above a property of the instance KIND and not of the operations — and a request
/// an organization instance cannot serve is refused for ITS OWN reason, never with the
/// account-instance sentence.
#[tokio::test]
async fn an_organization_instance_serves_the_same_region_writes() {
    let (router, instance_arn) = organization_router();

    let (status, added) = call(
        &router,
        "AddRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": "us-west-2"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "AddRegion on an organization instance expected 200, got {status} {added}"
    );
    assert_eq!(
        added["Status"].as_str(),
        Some("ADDING"),
        "the add answers the ADDING status, got {added}"
    );

    // Its own primary Region still cannot be removed — the rule the account instance never
    // reaches, here answered with the primary-Region wording instead.
    let (router, instance_arn) = organization_router();
    let (status, refused) = call(
        &router,
        "RemoveRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": PRIMARY_REGION}),
    )
    .await;
    assert_eq!(
        status, 400,
        "RemoveRegion of the primary Region expected 400, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused}"
    );
    let message = message_of(&refused);
    assert!(
        message.contains(PRIMARY_REGION) && message.contains("cannot be removed"),
        "an organization instance answers the primary-Region rule, got {message:?}"
    );
    assert_ne!(
        message,
        state::ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE,
        "an organization instance must not answer the account-instance restriction, got \
         {message:?}"
    );
}
