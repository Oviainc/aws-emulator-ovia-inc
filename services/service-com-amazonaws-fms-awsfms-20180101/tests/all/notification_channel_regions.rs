// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH notification channel a notification-channel call
//! acts on.
//!
//! The rule asserted here, in one sentence: the SNS notification channel is a
//! regional configuration whose three operations model no member that could name
//! one (`smithy_query(operation="GetNotificationChannel,DeleteNotificationChannel")`
//! reports an empty input for both), so the Region the request is signed for is
//! what selects the channel the call designates, reports or removes — each Region
//! of the account holds its own channel independently, and a Region that
//! designated none has none to report or remove however many other Regions have
//! one.
//!
//! Every case drives the service over the WIRE through the shared harness in
//! `policy_lifecycle` (an awsJson1_1 `POST /` with an `X-Amz-Target` header and a
//! SigV4 credential scope, whose scope carries the Region), and every wire
//! assertion is backed by the persisted record read through
//! `notification_channel_deletion::stored_channel`.
//!
//! The table in [`rows`] is the class the rule ranges over: it pairs the
//! designated-value rows of the sibling `notification_channel_deletion` table
//! (values at both published bounds of `com.amazonaws.fms#ResourceArn`, differing
//! in service and Region segment, role path form and punctuation) with Regions
//! that differ in their own form and length — including `us-east-1`, the
//! emulator's default Region, so a channel designated there cannot be confused
//! with the fallback an unsigned request would take, and including a Region that
//! designates nothing at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::notification_channel_deletion::{cases, stored_channel, Case, OTHER_REGION};
use crate::policy_lifecycle::{call_in, emulator_with_engine, error_code, REGION};

/// The Regions the rows designate a channel in, one per row of [`cases`]:
/// `REGION` and `OTHER_REGION` are the sibling suite's two, and the rest add a
/// third partition-wide spread — `us-east-1` is the emulator's own default
/// Region (`state::DEFAULT_REGION`), the Region an UNSIGNED request would fall
/// back to, so designating there is the case that would hide a handler reading
/// the fallback instead of the request's scope.
const DESIGNATED_REGIONS: [&str; 5] = [
    REGION,
    OTHER_REGION,
    "ap-southeast-1",
    "us-east-1",
    "me-south-1",
];

/// The emulator's own default Region (`DEFAULT_REGION` in `src/state.rs`): the
/// Region a request that names none addresses. One of [`DESIGNATED_REGIONS`], so
/// the fallback cases below read a channel that was really designated.
const DEFAULT_REGION: &str = "us-east-1";

/// A Region that designates NO channel, so what it reports and what it can
/// delete is the "no channel here" answer rather than another Region's channel.
const UNDESIGNATED_REGION: &str = "sa-east-1";

/// One row: the Region a call is signed for, and the pair of values designated
/// there.
struct Row {
    region: &'static str,
    case: Case,
}

impl Row {
    /// Names the row in every assertion message.
    fn name(&self) -> String {
        format!("{} / {}", self.region, self.case.name)
    }

    /// The pair [`stored_channel`] reports for a designated row.
    fn stored(&self) -> Option<(String, String)> {
        Some((self.case.topic.clone(), self.case.role.clone()))
    }
}

/// The table: each Region paired with one row of the shared value class.
fn rows() -> Vec<Row> {
    DESIGNATED_REGIONS
        .into_iter()
        .zip(cases())
        .map(|(region, case)| Row { region, case })
        .collect()
}

/// Designate a channel, signed for `region`.
async fn put_channel_in(
    router: &MultiServiceRouter,
    region: &str,
    topic: &str,
    role: &str,
) -> (u16, Value) {
    call_in(
        router,
        "PutNotificationChannel",
        &json!({ "SnsTopicArn": topic, "SnsRoleName": role }),
        region,
    )
    .await
}

/// Read the channel back, signed for `region`.
async fn get_channel_in(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call_in(router, "GetNotificationChannel", &json!({}), region).await
}

/// Delete the channel, signed for `region`.
async fn delete_channel_in(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call_in(router, "DeleteNotificationChannel", &json!({}), region).await
}

/// An emulator with the administrator gate open and every row of the table
/// designated in its own Region.
async fn emulator_with_every_row_designated(
) -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    for row in rows() {
        let name = row.name();
        let (status, body) =
            put_channel_in(&router, row.region, &row.case.topic, &row.case.role).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected PutNotificationChannel to answer 200, got {status} with \
             {body}"
        );
    }
    (router, engine)
}

/// Assert that `region` reports no channel, through the reader and in state.
async fn assert_no_channel(
    router: &MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    region: &str,
    case: &str,
) {
    let (status, body) = get_channel_in(router, region).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: a Region with no channel designated must report none, got {status} with \
         {body}"
    );
    let stored = stored_channel(engine, region);
    assert_eq!(
        stored, None,
        "case {case:?}: a Region with no channel designated must hold none, got {stored:?}"
    );
}

/// The rule for the reader: with every row designated in its own Region, each
/// Region reports the channel IT designated — never another Region's — and a
/// Region that designated none reports the modelled not-found.
#[tokio::test]
async fn each_region_reports_the_channel_it_designated() {
    let (router, engine) = emulator_with_every_row_designated().await;

    for row in rows() {
        let name = row.name();
        let (status, got) = get_channel_in(&router, row.region).await;
        assert_eq!(
            (
                status,
                got["SnsTopicArn"].as_str(),
                got["SnsRoleName"].as_str()
            ),
            (
                200,
                Some(row.case.topic.as_str()),
                Some(row.case.role.as_str())
            ),
            "case {name:?}: the Region must report its own designated channel, got {status} with \
             {got}"
        );
        let stored = stored_channel(&engine, row.region);
        assert_eq!(
            stored,
            row.stored(),
            "case {name:?}: the Region's own channel must be the persisted one, got {stored:?}"
        );
    }

    assert_no_channel(&router, &engine, UNDESIGNATED_REGION, UNDESIGNATED_REGION).await;

    // One channel per Region, and not one more: the designations are independent
    // records, so the store holds exactly the Regions that designated one.
    let keys = with_store(&engine, |store| {
        store
            .notification_channels
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    });
    let mut expected = DESIGNATED_REGIONS.map(str::to_string).to_vec();
    expected.sort();
    assert_eq!(
        keys, expected,
        "every Region that designated a channel must hold exactly one, got {keys:?}"
    );
}

/// The rule for the writer: a designation replaces the channel of the Region it
/// is signed for and of no other Region — so re-designating one Region's channel
/// leaves every other Region reporting what it designated itself.
#[tokio::test]
async fn a_designation_replaces_only_the_signed_regions_channel() {
    let table = rows();
    for index in 0..table.len() {
        let (router, engine) = emulator_with_every_row_designated().await;
        let table = rows();
        let target = &table[index];
        // Re-designate the target Region with the NEXT row's values, so the new
        // pair is one the table already covers and is different from the target's
        // own.
        let replacement = &table[(index + 1) % table.len()].case;
        let name = format!("{} replaced by {}", target.name(), replacement.name);

        let (status, body) = put_channel_in(
            &router,
            target.region,
            &replacement.topic,
            &replacement.role,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the re-designation to answer 200, got {status} with {body}"
        );

        let (status, got) = get_channel_in(&router, target.region).await;
        assert_eq!(
            (
                status,
                got["SnsTopicArn"].as_str(),
                got["SnsRoleName"].as_str()
            ),
            (
                200,
                Some(replacement.topic.as_str()),
                Some(replacement.role.as_str())
            ),
            "case {name:?}: the re-designated Region must report the new channel, got {status} \
             with {got}"
        );

        for other in table.iter().filter(|row| row.region != target.region) {
            let stored = stored_channel(&engine, other.region);
            assert_eq!(
                stored,
                other.stored(),
                "case {name:?}: Region {} must keep the channel it designated, got {stored:?}",
                other.region
            );
            let (status, got) = get_channel_in(&router, other.region).await;
            assert_eq!(
                (
                    status,
                    got["SnsTopicArn"].as_str(),
                    got["SnsRoleName"].as_str()
                ),
                (
                    200,
                    Some(other.case.topic.as_str()),
                    Some(other.case.role.as_str())
                ),
                "case {name:?}: Region {} must still report its own channel, got {status} with \
                 {got}",
                other.region
            );
        }
    }
}

/// The rule for the delete: it removes the channel of the Region it is signed for
/// and of no other Region — and a delete signed for a Region that designated none
/// finds no channel resource to remove, whatever other Regions hold.
#[tokio::test]
async fn deleting_removes_only_the_signed_regions_channel() {
    let (router, engine) = emulator_with_every_row_designated().await;
    let table = rows();

    // A Region with nothing designated has nothing to delete, and the delete
    // writes nothing: every other Region's channel survives it.
    let (status, body) = delete_channel_in(&router, UNDESIGNATED_REGION).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "a delete in a Region that designated no channel must be refused, got {status} with {body}"
    );
    for row in &table {
        let stored = stored_channel(&engine, row.region);
        assert_eq!(
            stored,
            row.stored(),
            "case {:?}: the refused delete must leave every other Region's channel stored, got \
             {stored:?}",
            row.name()
        );
    }

    // Then one Region at a time: its own channel goes, and only its own.
    for (index, row) in table.iter().enumerate() {
        let name = row.name();
        let (status, body) = delete_channel_in(&router, row.region).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected DeleteNotificationChannel to answer 200, got {status} with \
             {body}"
        );
        assert_no_channel(&router, &engine, row.region, &name).await;

        for later in &table[index + 1..] {
            let stored = stored_channel(&engine, later.region);
            assert_eq!(
                stored,
                later.stored(),
                "case {name:?}: Region {} must keep its channel through another Region's delete, \
                 got {stored:?}",
                later.region
            );
        }
    }

    let remaining = with_store(&engine, |store| store.notification_channels.len());
    assert_eq!(
        remaining, 0,
        "every designated channel was deleted in its own Region, got {remaining} left"
    );
}

/// A store keyed by a value the request carries is bounded: once the bound of
/// Regions hold a channel, designating one in a FURTHER Region is refused with
/// the error this operation models for a request it cannot carry out, while
/// REPLACING the channel of a Region that already holds one adds no record and
/// is never refused.
#[tokio::test]
async fn the_number_of_regions_holding_a_channel_is_bounded() {
    // The bound the handler enforces, read from the handler itself so the two
    // cannot drift.
    use service_com_amazonaws_fms_awsfms_20180101::ops::put_notification_channel::MAX_NOTIFICATION_CHANNEL_REGIONS;

    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let case = &cases()[0];

    for index in 0..MAX_NOTIFICATION_CHANNEL_REGIONS {
        let region = format!("eu-bound-{index}");
        let (status, body) = put_channel_in(&router, &region, &case.topic, &case.role).await;
        assert_eq!(
            status, 200,
            "case designation #{index} in {region}: expected 200 below the store bound, got \
             {status} with {body}"
        );
    }
    let held = with_store(&engine, |store| store.notification_channels.len());
    assert_eq!(
        held, MAX_NOTIFICATION_CHANNEL_REGIONS,
        "every designation below the bound must hold its own Region's channel, got {held}"
    );

    // One Region past the bound is refused, and the refusal stores nothing.
    let past = format!("eu-bound-{MAX_NOTIFICATION_CHANNEL_REGIONS}");
    let (status, body) = put_channel_in(&router, &past, &case.topic, &case.role).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidOperationException"),
        "case designation #{MAX_NOTIFICATION_CHANNEL_REGIONS} in {past}: designating past the \
         store bound must be refused, got {status} with {body}"
    );
    let stored = stored_channel(&engine, &past);
    assert_eq!(
        stored, None,
        "the refused designation must have stored no channel, got {stored:?}"
    );

    // A replace in a Region that already holds one is not a further Region, so
    // the bound does not refuse it.
    let replacement = &cases()[1];
    let occupied = "eu-bound-0";
    let (status, body) =
        put_channel_in(&router, occupied, &replacement.topic, &replacement.role).await;
    assert_eq!(
        status, 200,
        "replacing the channel of a Region that already holds one must not be refused, got \
         {status} with {body}"
    );
    let stored = stored_channel(&engine, occupied);
    assert_eq!(
        stored,
        Some((replacement.topic.clone(), replacement.role.clone())),
        "the replaced channel must be the one just designated, got {stored:?}"
    );
    let held = with_store(&engine, |store| store.notification_channels.len());
    assert_eq!(
        held, MAX_NOTIFICATION_CHANNEL_REGIONS,
        "a replace must add no record, got {held}"
    );
}

/// A credential scope whose Region is not a Region name names no Region this
/// service serves, so the call addresses the default Region — exactly as a
/// request carrying no credential scope does — rather than keying the store by
/// whatever the scope held.
#[tokio::test]
async fn a_scope_region_that_is_not_a_region_name_addresses_the_default_region() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let table = rows();
    let default_row = table
        .iter()
        .find(|row| row.region == DEFAULT_REGION)
        .expect("the table designates a channel in the default Region");

    let (status, body) = put_channel_in(
        &router,
        DEFAULT_REGION,
        &default_row.case.topic,
        &default_row.case.role,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the default Region's designation to answer 200, got {status} with {body}"
    );

    // Rows differ in WHY the scope names no Region: case, charset (an underscore,
    // a dot and a punctuation mark, none of which a Region name carries) and
    // length.
    let too_long = "a".repeat(33);
    let not_region_names = [
        "US-EAST-1",
        "us_east_1",
        "us.east.1",
        "us-east-1!",
        too_long.as_str(),
    ];
    let mut expected = (
        default_row.case.topic.clone(),
        default_row.case.role.clone(),
    );
    for (index, scope) in not_region_names.iter().enumerate() {
        // A read through such a scope reports what the DEFAULT Region holds.
        let (status, got) = get_channel_in(&router, scope).await;
        assert_eq!(
            (
                status,
                got["SnsTopicArn"].as_str(),
                got["SnsRoleName"].as_str()
            ),
            (200, Some(expected.0.as_str()), Some(expected.1.as_str())),
            "case {scope:?}: a scope naming no Region must report the default Region's channel, \
             got {status} with {got}"
        );

        // And a designation through it writes the DEFAULT Region's channel,
        // keying no record of its own: the default Region's own reader reports
        // what it wrote.
        let replacement = &table[(index + 1) % table.len()].case;
        let (status, body) =
            put_channel_in(&router, scope, &replacement.topic, &replacement.role).await;
        assert_eq!(
            status, 200,
            "case {scope:?}: expected the designation to answer 200, got {status} with {body}"
        );
        expected = (replacement.topic.clone(), replacement.role.clone());
        let stored = stored_channel(&engine, DEFAULT_REGION);
        assert_eq!(
            stored,
            Some(expected.clone()),
            "case {scope:?}: the designation must have written the default Region's channel, got \
             {stored:?}"
        );
        let keys = with_store(&engine, |store| {
            store
                .notification_channels
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        });
        assert_eq!(
            keys,
            vec![DEFAULT_REGION.to_string()],
            "case {scope:?}: a scope naming no Region must key no record of its own, got {keys:?}"
        );
    }
}
