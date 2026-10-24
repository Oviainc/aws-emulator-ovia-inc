// SPDX-License-Identifier: CC0-1.0

//! The wire form of this service's `@timestampFormat("date-time")` members.
//!
//! RULE: a structure member the model marks `@timestampFormat("date-time")` travels on
//! the awsJson1_0 wire as an RFC 3339 / ISO-8601 date-time STRING — never as the epoch
//! NUMBER the protocol's default timestamp encoding would produce — and the string names
//! exactly the instant the service holds for it.
//!
//! The members the rule bites on are every `Timestamp` member of this service's surface:
//! `smithy_query(query="operation",operation="GetAccountPlanState")` reports
//! `"timestampFormat": "date-time"` on `accountPlanExpirationDate`, and
//! `smithy_query(query="operation",operation="GetAccountActivity")` the same on
//! `expiresAt`, `startedAt` and `completedAt`. No other operation of this service reports
//! a timestamp at all (`ListAccountActivities`' `ActivitySummary`, `UpgradeAccountPlan`'s
//! response and `GetFreeTierUsage`' `FreeTierUsage` carry none), so these four members
//! are the whole class.
//!
//! The suite checks the rule twice over, at both ends of the mechanism that carries it:
//!
//! * on the WIRE, over a table of stored instants spanning the span a `Timestamp` can
//!   hold, including the round trip back to the instant the store holds (so a well-formed
//!   string naming the wrong moment fails too);
//! * on the SCHEMA, because the member schema's `timestamp_format()` is the single thing
//!   the JSON codec consults (`vendor/smithy-schema-server/src/json.rs::write_timestamp`)
//!   — with each shape's non-timestamp members as control rows.
//!
//! The harness (router, request signing, response parsing) is the one `free_tier.rs`
//! already builds; this suite drives it rather than copying it.

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::ops::types::{
    GetAccountActivityOutput, GetAccountPlanStateOutput,
};
use service_com_amazonaws_freetier_awsfreetierservice::state::{
    ComAmazonawsFreetierAwsfreetierserviceState, ACTIVITY_STATUS_COMPLETED,
    ACTIVITY_STATUS_IN_PROGRESS, PLAN_STATUS_ACTIVE, PLAN_TYPE_FREE,
};

use crate::free_tier::{harness, ok, text};

/// Assert `member` of `payload` is the RFC 3339 date-time string the rule requires, and
/// answer the instant it names in epoch milliseconds.
///
/// The failure messages name the two ways the rule is broken: a non-string (which is what
/// the protocol default's epoch NUMBER is) and a string no date-time parser accepts.
fn date_time_millis(case: &str, payload: &Value, member: &str) -> i64 {
    let value = payload
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} to be reported, got {payload}"));
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it must be \
             an RFC 3339 string and never the protocol-default epoch number, got {value}"
        )
    });
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: {member} = {rendered:?} must parse as an RFC 3339 date-time: {error}"
        )
    });
    // Smithy's `date-time` is RFC 3339 with a `Z` offset, so the two designators are part
    // of the form a generated SDK's parser is handed.
    assert!(
        rendered.contains('T') && rendered.ends_with('Z'),
        "case {case:?}: {member} must carry the T separator and the Z offset, got {rendered:?}"
    );
    parsed.to_millis().unwrap_or_else(|error| {
        panic!("case {case:?}: {member} = {rendered:?} is unrepresentable: {error}")
    })
}

/// RULE on `GetAccountPlanState$accountPlanExpirationDate`, over the span of instants a
/// stored plan can carry.
///
/// The rows differ along what the rule generalises over: where the instant sits relative
/// to the epoch (long before it, just before it, on it, after it, far after it), whether
/// it carries a fractional second at all, and whether that fraction's last digit is zero.
#[tokio::test]
async fn the_plan_expiration_date_is_an_rfc_3339_string_naming_the_stored_instant() {
    // (case, stored expiration instant in epoch milliseconds)
    let rows: [(&str, i64); 7] = [
        ("the Unix epoch itself", 0),
        ("one millisecond before the epoch", -1),
        ("an instant long before the epoch", -2_208_988_800_000),
        ("a whole second, with no fractional part", 1_700_000_000_000),
        ("a millisecond fraction", 1_792_729_178_653),
        ("a fraction whose last digit is zero", 1_792_729_178_650),
        ("a far-future instant", 4_102_444_800_123),
    ];

    for (case, expires_at_millis) in rows {
        let (router, engine) = harness();
        {
            let state = engine.state::<ComAmazonawsFreetierAwsfreetierserviceState>();
            let mut store = state.lock();
            let plan = store.plan_mut(engine::emulator_account_id());
            plan.plan_type = PLAN_TYPE_FREE.to_string();
            plan.plan_status = PLAN_STATUS_ACTIVE.to_string();
            plan.expiration_date = Some(DateTime::from_millis(expires_at_millis));
        }

        let payload = ok(&router, "GetAccountPlanState", &json!({})).await;
        let got = date_time_millis(case, &payload, "accountPlanExpirationDate");
        assert_eq!(
            got, expires_at_millis,
            "case {case:?}: accountPlanExpirationDate must name the stored instant \
             {expires_at_millis} ms, got {got} ms from {payload}"
        );
    }
}

/// RULE on `GetAccountActivity`'s three date-time members, over the same span of instants
/// and over every row of the catalog — so the rule is asserted for each member on rows
/// that differ in status, and therefore in which of the three members is reported at all.
#[tokio::test]
async fn every_activity_date_time_member_is_an_rfc_3339_string_naming_the_stored_instant() {
    // (case, the instant `expiresAt` is stored at, in epoch milliseconds). `startedAt`
    // and `completedAt` are stored one and two seconds later, so a serializer that mixed
    // two members up fails too.
    let rows: [(&str, i64); 6] = [
        ("the Unix epoch itself", 0),
        ("one millisecond before the epoch", -1),
        ("an instant long before the epoch", -2_208_988_800_000),
        ("a whole second, with no fractional part", 1_700_000_000_000),
        ("a millisecond fraction", 1_792_729_178_653),
        ("a far-future instant", 4_102_444_800_123),
    ];

    for (case, expires_at_millis) in rows {
        let (router, engine) = harness();
        let started_at_millis = expires_at_millis + 1_000;
        let completed_at_millis = expires_at_millis + 2_000;
        {
            let state = engine.state::<ComAmazonawsFreetierAwsfreetierserviceState>();
            let mut store = state.lock();
            for activity in store
                .activities
                .get_mut(engine::emulator_account_id())
                .expect("the seeded catalog is the caller's own")
            {
                activity.expires_at = Some(DateTime::from_millis(expires_at_millis));
                activity.started_at = Some(DateTime::from_millis(started_at_millis));
                activity.completed_at = Some(DateTime::from_millis(completed_at_millis));
            }
        }

        let listed = ok(&router, "ListAccountActivities", &json!({"maxResults": 1000})).await;
        let ids: Vec<String> = listed["activities"]
            .as_array()
            .expect("activities is a list")
            .iter()
            .map(|summary| text(summary, "activityId"))
            .collect();
        assert!(
            !ids.is_empty(),
            "case {case:?}: the catalog must hold at least one activity to assert the rule on"
        );

        for id in ids {
            let detail = ok(&router, "GetAccountActivity", &json!({"activityId": id})).await;
            let status = text(&detail, "status");
            let case = format!("{case} on activity {id} in {status}");

            // `expiresAt` is reported for every row; `startedAt` only for an activity
            // that has started and `completedAt` only for a completed one, which is the
            // handler's own rule — so a member the status withholds is checked to be
            // absent rather than to be a string.
            let expected: [(&str, Option<i64>); 3] = [
                ("expiresAt", Some(expires_at_millis)),
                (
                    "startedAt",
                    (status == ACTIVITY_STATUS_IN_PROGRESS || status == ACTIVITY_STATUS_COMPLETED)
                        .then_some(started_at_millis),
                ),
                (
                    "completedAt",
                    (status == ACTIVITY_STATUS_COMPLETED).then_some(completed_at_millis),
                ),
            ];

            for (member, expected_millis) in expected {
                match expected_millis {
                    Some(expected_millis) => {
                        let got = date_time_millis(&case, &detail, member);
                        assert_eq!(
                            got, expected_millis,
                            "case {case:?}: {member} must name the stored instant \
                             {expected_millis} ms, got {got} ms from {detail}"
                        );
                    }
                    None => assert!(
                        detail[member].is_null(),
                        "case {case:?}: {member} is withheld in this status, got {detail}"
                    ),
                }
            }
        }
    }
}

/// One member of a hand-written output schema: its index in the model's member list, its
/// wire name, whether the model marks it `@timestampFormat("date-time")`, and the schema
/// the service emits for it.
type MemberRow<'a> = (usize, &'a str, bool, &'a aws_smithy_schema::Schema);

/// One output shape: its name, its structure schema, and its member rows.
type ShapeRow<'a> = (&'a str, &'a aws_smithy_schema::Schema, &'a [MemberRow<'a>]);

/// RULE on the schema: every member the model marks `@timestampFormat("date-time")`
/// reports that format, and a member the model gives no `@timestampFormat` reports none —
/// which is what makes the codec write a string for the first kind and nothing special
/// for the rest.
///
/// This also pins the two hand-written schemas (see `src/ops/types/date_time_wire.rs` for
/// why they are hand-written) against the model: the wire name and the `member_index` of
/// every member, in the order `smithy_query(query="operation",...)` lists them.
#[test]
fn the_output_schemas_match_the_model_member_for_member() {
    // The model's own member list for `GetAccountPlanStateResponse` and
    // `GetAccountActivityResponse`, from
    // `smithy_query(query="operation",operation="GetAccountPlanState,GetAccountActivity")`.
    // The non-timestamp members are the control rows: they must report NO format, so the
    // assertion is about the trait and not about every member schema.
    let shapes: [ShapeRow<'_>; 2] = [
        (
            "GetAccountPlanStateResponse",
            &GetAccountPlanStateOutput::SCHEMA,
            &[
                (0, "accountId", false, &GetAccountPlanStateOutput::M_ACCOUNT_ID),
                (
                    1,
                    "accountPlanType",
                    false,
                    &GetAccountPlanStateOutput::M_ACCOUNT_PLAN_TYPE,
                ),
                (
                    2,
                    "accountPlanStatus",
                    false,
                    &GetAccountPlanStateOutput::M_ACCOUNT_PLAN_STATUS,
                ),
                (
                    3,
                    "accountPlanRemainingCredits",
                    false,
                    &GetAccountPlanStateOutput::M_ACCOUNT_PLAN_REMAINING_CREDITS,
                ),
                (
                    4,
                    "accountPlanExpirationDate",
                    true,
                    &GetAccountPlanStateOutput::M_ACCOUNT_PLAN_EXPIRATION_DATE,
                ),
            ],
        ),
        (
            "GetAccountActivityResponse",
            &GetAccountActivityOutput::SCHEMA,
            &[
                (0, "activityId", false, &GetAccountActivityOutput::M_ACTIVITY_ID),
                (1, "title", false, &GetAccountActivityOutput::M_TITLE),
                (2, "description", false, &GetAccountActivityOutput::M_DESCRIPTION),
                (3, "status", false, &GetAccountActivityOutput::M_STATUS),
                (
                    4,
                    "instructionsUrl",
                    false,
                    &GetAccountActivityOutput::M_INSTRUCTIONS_URL,
                ),
                (5, "reward", false, &GetAccountActivityOutput::M_REWARD),
                (
                    6,
                    "estimatedTimeToCompleteInMinutes",
                    false,
                    &GetAccountActivityOutput::M_ESTIMATED_TIME_TO_COMPLETE_IN_MINUTES,
                ),
                (7, "expiresAt", true, &GetAccountActivityOutput::M_EXPIRES_AT),
                (8, "startedAt", true, &GetAccountActivityOutput::M_STARTED_AT),
                (9, "completedAt", true, &GetAccountActivityOutput::M_COMPLETED_AT),
            ],
        ),
    ];

    for (shape, schema, members) in shapes {
        for (index, wire_name, is_date_time, member) in members {
            let case = format!("{shape}${wire_name}");
            let format = member.timestamp_format().map(|trait_| trait_.format());
            if *is_date_time {
                assert!(
                    matches!(format, Some(TimestampFormat::DateTime)),
                    "case {case:?}: the model marks this member @timestampFormat(\"date-time\"), \
                     so its schema must carry that format, got {format:?}"
                );
            } else {
                assert!(
                    format.is_none(),
                    "case {case:?}: the model gives this member no @timestampFormat, \
                     got {format:?}"
                );
            }
            assert_eq!(
                member.member_index(),
                Some(*index),
                "case {case:?}: the member must sit at the model's index {index}, got {:?}",
                member.member_index()
            );
            assert_eq!(
                member.member_name(),
                Some(*wire_name),
                "case {case:?}: the member's wire name must be the model's, got {:?}",
                member.member_name()
            );
        }

        // The structure's own members, read off `SCHEMA` rather than off the consts
        // above, so a member left out of the structure schema is caught too.
        let on_schema: Vec<Option<&str>> = schema
            .members()
            .iter()
            .map(|member| member.member_name())
            .collect();
        let expected: Vec<Option<&str>> = members
            .iter()
            .map(|(_, wire_name, _, _)| Some(*wire_name))
            .collect();
        assert_eq!(
            on_schema, expected,
            "case {shape:?}: SCHEMA must name every modelled member in order, got {on_schema:?}"
        );
    }
}
