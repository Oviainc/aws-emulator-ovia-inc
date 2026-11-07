// SPDX-License-Identifier: CC0-1.0

//! The version a REFUSED submission of a procurement-portal validation code leaves is kept
//! by every later READ of the preference, driven end to end over this service's wire
//! protocol with the shared harness of `refused_procurement_portal_writes`.
//!
//! The rule under test: a reader reports the state the last WRITE left and performs none of
//! its own, so reading a preference — through `GetProcurementPortalPreference`, through the
//! listing, or through the supplier walk the same preferences feed — can never move the
//! `Version` or the `LastUpdateDate` a refused submission's recorded attempt left behind
//! (`smithy_query(query="shape", shape="ProcurementPortalPreference")`: `LastUpdateDate` is
//! "the date and time when the procurement portal preference was last updated"). Which
//! preference the refused submission wrote at all is the rule
//! `rejected_validation_code_leaves_other_preferences_alone` spans.
//!
//! The rows are how many submissions were refused before the reads, so the version held
//! across them is not one a single attempt happens to leave.

use serde_json::{json, Value};

use crate::refused_procurement_portal_writes::{
    arn_of, call, create_with_delivery_endpoint, emulator_with_engine, get_preference,
    issue_code, last_update, listed_summary, ok, shifted, version, PORTAL_NAMES,
};

/// How many times each row reads the preference back after the refusals: more than once,
/// because a read that writes is only visible in the SECOND read after it.
const READS: usize = 3;

/// RULE: a reader of a procurement portal preference reports what the last write left and
/// writes nothing itself, so the `Version` and `LastUpdateDate` a refused code submission
/// left are the ones every later read reports — however many reads, and through whichever
/// reader of the plane.
#[tokio::test]
async fn the_version_a_rejected_code_left_survives_every_read() {
    let (router, engine) = emulator_with_engine();

    for (index, refused_submissions) in [1_usize, 2, 4].into_iter().enumerate() {
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_with_delivery_endpoint(
                    portal,
                    &format!("BUYER-READS-{index}"),
                    &format!("SUPPLIER-READS-{index}"),
                ),
            )
            .await,
        );
        let issued = issue_code(&router, &engine, &arn).await;
        let wrong = shifted(&issued);

        for attempt in 1..=refused_submissions {
            let (status, body) = call(
                &router,
                "VerifyProcurementPortalValidation",
                &json!({"ProcurementPortalPreferenceArn": arn, "Code": wrong}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {portal} attempt {attempt}: expected HTTP 400, got {status} {body}"
            );
        }

        // What those refusals left: the create, the issued code and one write per recorded
        // attempt.
        let left = get_preference(&router, &arn).await;
        let expected_version = 2 + refused_submissions as i64;
        assert_eq!(
            version(&left),
            expected_version,
            "case {portal}: the create, the issued code and {refused_submissions} recorded \
             attempts leave Version {expected_version}, got {left}"
        );

        // And no reader moves it, however often it is called.
        for round in 1..=READS {
            let read = get_preference(&router, &arn).await;
            assert_eq!(
                read, left,
                "case {portal} read {round}: a read reports what the last write left and \
                 writes nothing, got {read}"
            );

            let summary = listed_summary(&router, &arn).await;
            assert_eq!(
                version(&summary),
                version(&left),
                "case {portal} read {round}: the listing reports the version the read does, \
                 got {summary}"
            );
            assert_eq!(
                last_update(&summary),
                last_update(&left),
                "case {portal} read {round}: the listing reports the instant the read does, \
                 got {summary}"
            );

            // The supplier walk is served from the same stored preferences, so it is a
            // reader of them too.
            let suppliers = ok(
                &router,
                "ListProcurementPortalSuppliers",
                &json!({"PortalIdentifier": portal.to_ascii_lowercase().replace('_', "-")}),
            )
            .await;
            assert!(
                suppliers.get("ProcurementPortalSuppliers").is_some(),
                "case {portal} read {round}: the supplier walk answers a list, got {suppliers}"
            );
            let after_walk = get_preference(&router, &arn).await;
            assert_eq!(
                after_walk, left,
                "case {portal} read {round}: walking the suppliers of a portal writes none \
                 of its preferences, got {after_walk}"
            );
        }

        // The code the refusals were measured against is still the outstanding one, so the
        // reads did not consume it either.
        let completed = ok(
            &router,
            "VerifyProcurementPortalValidation",
            &json!({"ProcurementPortalPreferenceArn": arn, "Code": issued}),
        )
        .await;
        assert_eq!(
            arn_of(&completed),
            arn,
            "case {portal}: the issued code still completes the validation after the reads, \
             got {completed}"
        );
        let after_verify = get_preference(&router, &arn).await;
        assert_eq!(
            version(&after_verify),
            expected_version + 1,
            "case {portal}: the completed validation is the next version after what the \
             reads left, got {after_verify}"
        );
        assert_ne!(
            after_verify.get("EinvoiceDeliveryPreferenceStatus"),
            Option::<&Value>::None,
            "case {portal}: the validated plane reports a status, got {after_verify}"
        );
    }
}
