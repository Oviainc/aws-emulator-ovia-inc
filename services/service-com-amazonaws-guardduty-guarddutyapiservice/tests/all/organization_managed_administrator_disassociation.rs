// SPDX-License-Identifier: CC0-1.0

//! The organization precondition on the MEMBER side of the GuardDuty association:
//! `DisassociateFromAdministratorAccount` and its deprecated twin
//! `DisassociateFromMasterAccount`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch` (the
//! harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test, asserted over a SET of inputs rather than one call: while the
//! organization configuration stored for the detector a request names auto-enables
//! GuardDuty for EVERY account of the organization, a call that would end that
//! account's GuardDuty relationship is refused with HTTP 400 under the
//! `InvalidInputException` class and writes nothing — "with
//! `autoEnableOrganizationMembers` configuration for your organization set to `ALL`,
//! you'll receive an error if you attempt to disable GuardDuty in a member account"
//! (`smithy_query(query="doc",shape="DisassociateFromAdministratorAccount")`). It is
//! the same precondition the administrator-side operations enforce (the
//! `organization_managed_members` suite owns those), here on the side of the
//! relationship that names no member account.
//!
//! The cases span it along:
//!
//! 1. the stored `AutoEnableMembers` value — only `ALL` refuses, `NEW` and `NONE`
//!    disassociate — crossed with both names the disassociation is bound under and
//!    with the Region the detector lives in;
//! 2. the view that wrote the value — the deprecated boolean `autoEnable` cannot say
//!    `ALL`, so a configuration written through it never refuses;
//! 3. the detector — the precondition is the NAMED detector's own configuration, so
//!    one detector's `ALL` does not refuse another's call;
//! 4. the order of the checks — the precondition holds for the whole call, so it
//!    refuses an account holding no relationship at all too, while a `DetectorId`
//!    this account/Region does not hold is still refused as the detector failure it
//!    is;
//! 5. reversibility — lifting the configuration lets the same call through, which is
//!    what proves the refusal left the relationship where it was.
//!
//! The invitations are seeded through this service's own state module (the
//! `negative_class_rejections` suite owns that helper), because the operation that
//! delivers one — `InviteMembers`, called by ANOTHER account — cannot be reached by
//! this single-account emulator's own requests.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_guardduty_guarddutyapiservice::state::{now_millis, DETECTOR_ID_MEMBER};

use crate::detector_lifecycle::{
    call_in, error_code, error_shape, router_over, Router, ABSENT_DETECTOR_ID,
};
use crate::invitation_listing::enabled_detector_in;
use crate::negative_class_rejections::seed_invitation;
// Every `AutoEnableMembers` member paired with whether an organization configured
// that way manages the relationships of the accounts already in it: the table the
// administrator-side suite spans the same rule over, so the two sides cannot drift
// on which value is the managing one.
use crate::organization_managed_members::AUTO_ENABLE_MEMBERS;
// The organization-configuration harness lives once, in the suite that owns those
// two operations.
use crate::organization_configuration::{describe_in, update_ok_in};

/// The error CLASS every GuardDuty client rejection carries.
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, as `x-amzn-errortype`
/// carries it.
const ENVELOPE: &str = "BadRequestException";

/// The two names the member-side DISASSOCIATION is bound under, as path segments:
/// `POST /detector/{DetectorId}/master/disassociate` and
/// `.../administrator/disassociate`
/// (`smithy_query(operation="DisassociateFromMasterAccount,DisassociateFromAdministratorAccount")`).
/// They are one behaviour, so every case runs through both.
const DISASSOCIATIONS: [&str; 2] = ["master", "administrator"];

/// The two names the administrator READ is bound under, as `(path segment, output
/// member)` — both report the one stored relationship.
const READS: [(&str, &str); 2] = [("master", "master"), ("administrator", "administrator")];

/// The message the refusal carries, as the rule states it: the account cannot leave
/// while the organization auto-enables GuardDuty for all of its accounts, and the
/// remedy the documentation gives is to take the account out of the organization.
/// Written out here rather than read from the crate, so the assertion states the
/// rule instead of comparing the handler against itself.
const ORGANIZATION_REFUSAL: &str = "The request is rejected because the current account cannot disassociate from its administrator account while autoEnableOrganizationMembers is set to ALL for the organization. Remove the account from the organization first.";

/// Scenarios to run the whole table over: the Region the detector lives in, the
/// administrator account id across the 12-digit account-id space, and the shape of
/// the invitation id that established the relationship.
const SCENARIOS: [(&str, &str, &str); 3] = [
    (
        "us-east-1",
        "111122223333",
        "d4b94fc952d6912b8f3060768220e2e7",
    ),
    (
        "eu-west-1",
        "999999999999",
        "11111111-2222-3333-4444-555555555555",
    ),
    ("ap-southeast-2", "000000000000", "i"),
];

// ---------------------------------------------------------------------------
// Rules 1 and 5: only `ALL` refuses, through both names, and lifting it lets the
// same call through.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_an_organization_that_auto_enables_all_members_refuses_the_disassociation() {
    for (region, administrator_id, invitation_id) in SCENARIOS {
        for (value, managed) in AUTO_ENABLE_MEMBERS {
            for segment in DISASSOCIATIONS {
                let case =
                    format!("{region} /{segment}/disassociate with autoEnableOrganizationMembers {value}");
                let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
                let router = router_over(Arc::clone(&engine));
                let detector_id = enabled_detector_in(&router, region).await;

                update_ok_in(
                    &router,
                    region,
                    &detector_id,
                    json!({"autoEnableOrganizationMembers": value}),
                )
                .await;
                let reported = describe_in(&router, region, &detector_id).await;
                assert_eq!(
                    reported["autoEnableOrganizationMembers"],
                    json!(value),
                    "case {case}: expected the configuration stored, got {reported:?}"
                );

                establish(
                    &engine,
                    &router,
                    region,
                    &detector_id,
                    administrator_id,
                    invitation_id,
                )
                .await;

                let (status, body) = disassociate(&router, region, segment, &detector_id).await;
                if !managed {
                    assert_eq!(
                        status, 200,
                        "case {case}: an organization that does not auto-enable all members takes no relationship over, expected 200, got {status} {body:?}"
                    );
                    assert_no_relationship(&case, &router, region, &detector_id).await;
                    continue;
                }

                assert_organization_refusal(&case, status, &body);
                // The refusal wrote nothing: both reads still report the
                // relationship the acceptance established.
                assert_relationship(&case, &router, region, &detector_id, administrator_id).await;

                // And the configuration is what gated it: with the organization no
                // longer auto-enabling every account, the same call succeeds and the
                // relationship is gone.
                update_ok_in(
                    &router,
                    region,
                    &detector_id,
                    json!({"autoEnableOrganizationMembers": "NONE"}),
                )
                .await;
                let (status, body) = disassociate(&router, region, segment, &detector_id).await;
                assert_eq!(
                    status, 200,
                    "case {case}: once the configuration is lifted the same call must succeed, got {status} {body:?}"
                );
                assert_no_relationship(&case, &router, region, &detector_id).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the deprecated boolean view cannot express the managing value.
// ---------------------------------------------------------------------------

/// The deprecated `autoEnable` flag is a boolean, and the value it says yes with
/// covers the accounts that JOIN from then on rather than the ones already in the
/// organization (`smithy_query(query="doc",shape="UpdateOrganizationConfigurationRequest",member="AutoEnable")`),
/// so no configuration written through it manages an existing relationship.
#[tokio::test]
async fn a_configuration_written_through_the_deprecated_flag_never_refuses() {
    let region = "us-east-1";
    let administrator_id = "444455556666";
    let invitation_id = "0123456789abcdef0123456789abcdef";
    for flag in [true, false] {
        for segment in DISASSOCIATIONS {
            let case = format!("/{segment}/disassociate with autoEnable {flag}");
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let router = router_over(Arc::clone(&engine));
            let detector_id = enabled_detector_in(&router, region).await;
            update_ok_in(&router, region, &detector_id, json!({"autoEnable": flag})).await;
            let reported = describe_in(&router, region, &detector_id).await;
            assert_ne!(
                reported["autoEnableOrganizationMembers"],
                json!("ALL"),
                "case {case}: the deprecated flag cannot store the managing value, got {reported:?}"
            );
            establish(
                &engine,
                &router,
                region,
                &detector_id,
                administrator_id,
                invitation_id,
            )
            .await;

            let (status, body) = disassociate(&router, region, segment, &detector_id).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {body:?}"
            );
            assert_no_relationship(&case, &router, region, &detector_id).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the precondition is the NAMED detector's own configuration.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_detectors_configuration_does_not_refuse_another_detectors_call() {
    // A detector is one per account per Region, so the second one is reached through
    // another Region's endpoint.
    let managing_region = "us-east-1";
    let other_region = "us-west-2";
    let administrator_id = "777788889999";
    let invitation_id = "d4b94fc952d6912b8f3060768220e2e7";

    for segment in DISASSOCIATIONS {
        let case = format!("/{segment}/disassociate across two detectors");
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));

        let managing = enabled_detector_in(&router, managing_region).await;
        let other = enabled_detector_in(&router, other_region).await;
        update_ok_in(
            &router,
            managing_region,
            &managing,
            json!({"autoEnableOrganizationMembers": "ALL"}),
        )
        .await;
        for (region, detector_id) in [(managing_region, &managing), (other_region, &other)] {
            establish(
                &engine,
                &router,
                region,
                detector_id,
                administrator_id,
                invitation_id,
            )
            .await;
        }

        // The detector whose own configuration says nothing about auto-enablement
        // disassociates.
        let (status, body) = disassociate(&router, other_region, segment, &other).await;
        assert_eq!(
            status, 200,
            "case {case}: the other detector's call must not be refused, got {status} {body:?}"
        );
        assert_no_relationship(&case, &router, other_region, &other).await;

        // The one whose configuration auto-enables every account is refused, and its
        // relationship stays.
        let (status, body) = disassociate(&router, managing_region, segment, &managing).await;
        assert_organization_refusal(&case, status, &body);
        assert_relationship(&case, &router, managing_region, &managing, administrator_id).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the order of the checks.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_precondition_holds_for_the_whole_call_and_leaves_the_detector_failures_alone() {
    let region = "us-east-1";
    let administrator_id = "222233334444";
    let invitation_id = "11111111-2222-3333-4444-555555555555";

    for segment in DISASSOCIATIONS {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let detector_id = enabled_detector_in(&router, region).await;

        // The no-relationship refusal, read while nothing is auto-enabled: the rows
        // below are compared against it rather than only against the organization
        // message, so the two refusals are told apart from EACH OTHER.
        let case = format!("/{segment}/disassociate with no relationship and no configuration");
        let (status, body) = disassociate(&router, region, segment, &detector_id).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let no_relationship = message(&body);
        assert_ne!(
            no_relationship, ORGANIZATION_REFUSAL,
            "case {case}: a detector with no relationship and no configuration is not the organization refusal, got {body:?}"
        );

        update_ok_in(
            &router,
            region,
            &detector_id,
            json!({"autoEnableOrganizationMembers": "ALL"}),
        )
        .await;

        // An account holding NO relationship: the precondition is read before the
        // relationship is, so the call is refused for the organization rather than
        // for the absence — and an invitation in the inbox is still accepted
        // afterwards, which is what shows the refusal consumed nothing.
        let case = format!("/{segment}/disassociate with no relationship under ALL");
        let (status, body) = disassociate(&router, region, segment, &detector_id).await;
        assert_organization_refusal(&case, status, &body);
        establish(
            &engine,
            &router,
            region,
            &detector_id,
            administrator_id,
            invitation_id,
        )
        .await;

        // A `DetectorId` this account/Region does not hold is still the detector
        // failure: the organization configuration of some other detector cannot
        // answer for an id this endpoint does not serve.
        let held_elsewhere = enabled_detector_in(&router, "eu-central-1").await;
        let rows: [(&str, &str); 4] = [
            (ABSENT_DETECTOR_ID, "32 hex characters nobody minted"),
            (
                held_elsewhere.as_str(),
                "a detector another Region's endpoint holds",
            ),
            ("not-a-detector-id", "not hexadecimal at all"),
            ("abc123", "too short to be a detector id"),
        ];
        for (candidate, why) in rows {
            let case = format!("/{segment}/disassociate on {candidate} ({why})");
            let (status, body) = disassociate(&router, region, segment, candidate).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} {body:?}"
            );
            let message = message(&body);
            assert_ne!(
                message, ORGANIZATION_REFUSAL,
                "case {case}: expected the detector failure, got {body:?}"
            );
            assert!(
                message.contains(DETECTOR_ID_MEMBER),
                "case {case}: expected a message naming {DETECTOR_ID_MEMBER:?}, got {message:?}"
            );
        }

        // And the relationship the named detector holds is still there, so none of
        // the refusals above wrote anything.
        assert_relationship("after the refusals", &router, region, &detector_id, administrator_id)
            .await;
    }
}

// ---------------------------------------------------------------------------
// The calls and the shared assertions.
// ---------------------------------------------------------------------------

/// `POST /detector/{DetectorId}/{segment}/disassociate`, through `region`'s endpoint.
async fn disassociate(
    router: &Router,
    region: &str,
    segment: &str,
    detector_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/{segment}/disassociate"),
        None,
    )
    .await
}

/// Put a pending invitation in the detector's inbox and accept it, so there is a
/// relationship to leave, failing the test unless the acceptance succeeded.
async fn establish(
    engine: &engine::InMemoryEngine,
    router: &Router,
    region: &str,
    detector_id: &str,
    administrator_id: &str,
    invitation_id: &str,
) {
    seed_invitation(
        engine,
        region,
        detector_id,
        administrator_id,
        invitation_id,
        now_millis(),
    );
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/administrator"),
        Some(json!({"administratorId": administrator_id, "invitationId": invitation_id})),
    )
    .await;
    assert_eq!(
        status, 200,
        "accepting the invitation in {region}: expected 200, got {status} {body:?}"
    );
}

/// The administrator relationship one of the two reads reports.
async fn administrator_account(
    router: &Router,
    region: &str,
    segment: &str,
    output_member: &str,
    detector_id: &str,
) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/{segment}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "reading /{segment} in {region}: expected 200, got {status} {body:?}"
    );
    body[output_member].clone()
}

/// Assert both reads report the relationship `administrator_id` established.
async fn assert_relationship(
    case: &str,
    router: &Router,
    region: &str,
    detector_id: &str,
    administrator_id: &str,
) {
    for (read, member) in READS {
        let reported = administrator_account(router, region, read, member, detector_id).await;
        assert_eq!(
            reported["accountId"],
            json!(administrator_id),
            "case {case}: expected the relationship intact through /{read}, got {reported:?}"
        );
    }
}

/// Assert both reads report no administrator relationship at all: every member of
/// the reported shape is optional, so "none" is their ABSENCE.
async fn assert_no_relationship(case: &str, router: &Router, region: &str, detector_id: &str) {
    for (read, member) in READS {
        let reported = administrator_account(router, region, read, member, detector_id).await;
        assert_eq!(
            reported["accountId"],
            Value::Null,
            "case {case}: expected no administrator through /{read}, got {reported:?}"
        );
    }
}

/// Assert a response is the organization refusal: the classified 400 in all the
/// places the class appears, carrying the message the rule states.
fn assert_organization_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case}: expected the modelled envelope shape, got {body:?}"
    );
    assert_eq!(
        error_code(body),
        CLASS,
        "case {case}: expected the invalid-input class, got {body:?}"
    );
    assert_eq!(
        body["type"],
        json!(CLASS),
        "case {case}: expected the invalid-input class in the type member, got {body:?}"
    );
    assert_eq!(
        message(body),
        ORGANIZATION_REFUSAL,
        "case {case}: expected the organization refusal, got {body:?}"
    );
}

/// The `message` an error response carries.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}
