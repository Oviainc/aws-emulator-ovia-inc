// SPDX-License-Identifier: CC0-1.0

//! The status this service serves each error SHAPE with, and where the
//! partner-entitlement denial sits in the order an operation answers in.
//!
//! Two rules, each about every operation rather than about one, and each checked here as
//! a TABLE whose rows differ along what the rule generalises over:
//!
//! 1. **The served status of an error is a property of its SHAPE**: the shape's modelled
//!    `@httpError` for every error this service declares, except `AccessDeniedException`,
//!    which the service serves with [`DENIED_STATUS`]. The modelled statuses are listed
//!    in [`MODELLED_STATUS`], read from the model with
//!    `smithy_query(query="shape",shape="<Shape>",
//!    model_path="services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/smithy-model/com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice.json")`;
//!    the override is recorded behaviour (corpus `simparity-wire-aws-v1`, fixture
//!    `partnercentral-benefits-list-and-tag-validation-error-paths`, step `task#1`
//!    `ListBenefits` and step `task#4` `ListTagsForResource`), where the body is the
//!    modelled `AccessDeniedException` carrying the entitlement sentence and the status on
//!    the wire is 400.
//! 2. **The entitlement denial precedes every resource lookup**, so it is the WHOLE
//!    answer whatever the request named — the tag-READING operation included, which the
//!    same fixture records. The two tag-MUTATING operations are the one recorded
//!    exception (see `crate::service_error_rules`'s `UNGATED_OPERATIONS`), and the gate's
//!    per-operation table lives there; this suite spans the shapes and, for the recorded
//!    operation, the resource forms.
//!
//! Every assertion goes over the wire through the shared
//! [`crate::benefit_application_lifecycle::Harness`] (`MultiServiceRouter::dispatch`,
//! `awsJson1_0`, `POST /`, `X-Amz-Target: PartnerCentralBenefitsService.<Operation>`).

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    served_status as service_served_status, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, Harness};
use crate::resource_tagging::{application_arn, benefit_id, error_message, expected_arn, REGION};

/// The status this service serves `AccessDeniedException` with — the recorded one, not
/// the modelled 403. Written out here rather than read from the service's own constant so
/// the value itself is under test.
const DENIED_STATUS: u16 = 400;

/// The sentence the entitlement gate refuses with, written out here rather than read from
/// the service's own constant so the text itself is under test.
const ENTITLEMENT_MESSAGE: &str = "This action requires an active AWS partner benefit. \
Register as an AWS partner if you haven't already, or contact AWS partner central \
support.";

/// Every error shape this service declares, with the `@httpError` the model gives it.
/// Source: `smithy_query(query="shape",shape="<Shape>")` on this service's on-disk model
/// (see the module doc for the full call).
const MODELLED_STATUS: &[(&str, u16)] = &[
    ("AccessDeniedException", 403),
    ("ConflictException", 409),
    ("InternalServerException", 500),
    ("ResourceNotFoundException", 404),
    ("ServiceQuotaExceededException", 402),
    ("ThrottlingException", 429),
    ("ValidationException", 400),
];

/// The production catalog, one of the two the service serves.
const CATALOG: &str = "AWS";

/// An application id the modelled `BenefitApplicationIdentifier` admits and no create
/// ever minted, so every lookup of it finds nothing.
const UNKNOWN_APPLICATION: &str = "benappl-00000000000000";

/// A benefit id the modelled `BenefitId` admits and no catalog holds.
const UNKNOWN_BENEFIT: &str = "ben-00000000000000";

/// An allocation id the modelled `BenefitAllocationIdentifier` admits.
const UNKNOWN_ALLOCATION: &str = "benalloc-00000000000001";

/// Rule 1, as this suite states it: the status the service must serve `shape` with, from
/// the modelled catalogue above and the one recorded override.
fn served_status(shape: &str) -> u16 {
    let modelled = modelled_status(shape);
    if shape == "AccessDeniedException" {
        DENIED_STATUS
    } else {
        modelled
    }
}

/// The `@httpError` the model declares for `shape`.
fn modelled_status(shape: &str) -> u16 {
    MODELLED_STATUS
        .iter()
        .find(|(name, _)| *name == shape)
        .map(|(_, status)| *status)
        .unwrap_or_else(|| panic!("{shape:?} is not an error shape this service declares"))
}

/// Rule 1 as the SERVICE implements it, over the whole catalogue of shapes: the service's
/// own `served_status` moves exactly one shape's status — the access denial's, to the
/// recorded [`DENIED_STATUS`] — and leaves every other at the status the model declares.
#[test]
fn only_the_access_denial_is_served_at_a_status_the_model_does_not_declare() {
    for (shape, modelled) in MODELLED_STATUS {
        let served = service_served_status(shape, *modelled);
        if *shape == "AccessDeniedException" {
            assert_eq!(
                served, DENIED_STATUS,
                "case {shape:?}: the access denial is served at {DENIED_STATUS}, got {served}"
            );
            assert_ne!(
                served, *modelled,
                "case {shape:?}: the recorded status must differ from the modelled one, \
                 else there is nothing to override"
            );
        } else {
            assert_eq!(
                served, *modelled,
                "case {shape:?}: a shape the recordings say nothing about keeps the \
                 modelled status {modelled}, got {served}"
            );
        }
        assert_eq!(
            served,
            served_status(shape),
            "case {shape:?}: the service's rule and this suite's statement of it disagree"
        );
    }
}

/// Rule 1 over the wire: each error shape this emulator can be made to answer carries the
/// status [`served_status`] gives it — so the access denial is 400 and nothing else moved.
///
/// The rows differ along the error SHAPE (the class the rule generalises over), along the
/// operation that produces it (a list, a get, a create, a recall, a stub, both tagging
/// directions) and along the caller's entitlement.
#[tokio::test]
async fn every_error_carries_the_status_this_service_serves_its_shape_with() {
    let entitled = Harness::new();
    let unentitled = Harness::unentitled();

    // One real application, so the `ConflictException` row has a resource whose state
    // refuses the transition (a `PENDING_SUBMISSION` application cannot be recalled).
    let application = application_arn(&entitled, "seed", CATALOG, "tok-status-table", &[]).await;
    let existing_benefit = benefit_id(&entitled, "seed", CATALOG).await;

    // (expected error shape, the caller is entitled, operation, body)
    let rows: Vec<(&str, bool, &str, Value)> = vec![
        // AccessDeniedException: the gate, on a read, a write, a stub and the tag-reading
        // operation — the last two being the recorded steps of the issue's fixture.
        (
            "AccessDeniedException",
            false,
            "ListBenefits",
            json!({ "Catalog": CATALOG }),
        ),
        (
            "AccessDeniedException",
            false,
            "ListTagsForResource",
            json!({ "resourceArn": application }),
        ),
        (
            "AccessDeniedException",
            false,
            "CreateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-status-denied",
                "BenefitIdentifier": UNKNOWN_BENEFIT,
                "Name": "a status probe",
            }),
        ),
        (
            "AccessDeniedException",
            false,
            "GetBenefitAllocation",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_ALLOCATION }),
        ),
        // ValidationException: the centralized walk, ahead of the gate either way.
        (
            "ValidationException",
            true,
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": "not-an-identifier!!" }),
        ),
        (
            "ValidationException",
            false,
            "ListBenefits",
            json!({ "Catalog": CATALOG, "MaxResults": 1000 }),
        ),
        // ResourceNotFoundException: a lookup that found nothing, for an entitled caller.
        (
            "ResourceNotFoundException",
            true,
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_APPLICATION }),
        ),
        (
            "ResourceNotFoundException",
            true,
            "ListTagsForResource",
            json!({ "resourceArn": expected_arn(CATALOG, "benefit", &existing_benefit) }),
        ),
        (
            "ResourceNotFoundException",
            true,
            "TagResource",
            json!({
                "resourceArn": expected_arn(CATALOG, "benefit-application", UNKNOWN_APPLICATION),
                "tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
        // ConflictException: the application exists, but its status refuses the recall.
        (
            "ConflictException",
            true,
            "RecallBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "Identifier": application,
                "Reason": "a status probe",
            }),
        ),
    ];

    for (shape, is_entitled, operation, body) in &rows {
        let harness = if *is_entitled { &entitled } else { &unentitled };
        let case = format!("{shape} from {operation} (entitled: {is_entitled})");
        let (status, out) = harness.call(operation, body).await;
        assert_eq!(
            error_code(&out),
            *shape,
            "case {case:?}: expected {shape}, got {out:?}"
        );
        assert_eq!(
            status,
            served_status(shape),
            "case {case:?}: expected status {}, got {status} with {out:?}",
            served_status(shape)
        );
    }

    // And a stub still answers 501, which is not a modelled shape at all: the override is
    // over the modelled errors only.
    let (status, out) = entitled
        .call(
            "GetBenefitAllocation",
            &json!({ "Catalog": CATALOG, "Identifier": UNKNOWN_ALLOCATION }),
        )
        .await;
    assert_eq!(
        status, 501,
        "case \"an unimplemented operation, entitled\": expected 501, got {status} {out:?}"
    );
}

/// Rule 2 on the operation the issue's fixture recorded: for an un-entitled caller every
/// `resourceArn` form is answered identically — the denial, at [`DENIED_STATUS`], with the
/// entitlement sentence — so the answer says nothing about which resources exist, while
/// the SAME ARNs from an entitled caller get the lookup's own answers (a tag set, or a
/// 404).
///
/// The rows span the ways an ARN of the modelled `TaggableResourceArn` form can resolve:
/// a resource that exists, an application id nothing holds, a benefit (which holds no tag
/// set), a catalog the service does not serve, every seeded catalog, and the existing
/// resource's ARN in another region, account or partition.
#[tokio::test]
async fn the_entitlement_denial_precedes_the_tag_lookup_for_every_resource_form() {
    let entitled = Harness::new();
    let unentitled = Harness::unentitled();

    let existing = application_arn(&entitled, "seed", CATALOG, "tok-gate-order", &[("env", "test")])
        .await;
    let benefit = benefit_id(&entitled, "seed", CATALOG).await;

    // (case, the ARN, whether an ENTITLED caller is served 200 for it)
    let mut rows: Vec<(String, String, bool)> = vec![
        ("a resource that exists".to_string(), existing.clone(), true),
        (
            "an application id nothing holds".to_string(),
            expected_arn(CATALOG, "benefit-application", UNKNOWN_APPLICATION),
            false,
        ),
        (
            "a benefit, which holds no tag set".to_string(),
            expected_arn(CATALOG, "benefit", &benefit),
            false,
        ),
        (
            "a catalog the service does not serve".to_string(),
            expected_arn("NotACatalog", "benefit-application", UNKNOWN_APPLICATION),
            false,
        ),
        (
            "the resource's ARN in another region".to_string(),
            existing.replace(REGION, "eu-west-1"),
            false,
        ),
        (
            "the resource's ARN under another account".to_string(),
            existing.replace(crate::resource_tagging::ACCOUNT, "210987654321"),
            false,
        ),
        (
            "the resource's ARN in another partition".to_string(),
            existing.replace("arn:aws:", "arn:aws-us-gov:"),
            false,
        ),
    ];
    // Every catalog the store serves, so the row set cannot drift from the seeded data.
    for catalog in SEEDED_CATALOGS {
        rows.push((
            format!("an absent application of catalog {catalog}"),
            expected_arn(catalog, "benefit-application", UNKNOWN_APPLICATION),
            false,
        ));
    }

    // The un-entitled answers: one and the same denial for every form.
    let mut answers = Vec::new();
    for (case, resource_arn, _) in &rows {
        let (status, out) = unentitled
            .call("ListTagsForResource", &json!({ "resourceArn": resource_arn }))
            .await;
        assert_eq!(
            status, DENIED_STATUS,
            "case {case:?}: an un-entitled caller must be refused with {DENIED_STATUS}, \
             got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {out:?}"
        );
        assert_eq!(
            error_message(&out),
            ENTITLEMENT_MESSAGE,
            "case {case:?}: expected the entitlement sentence, got {out:?}"
        );
        answers.push((status, error_code(&out), error_message(&out)));
    }
    let first = answers
        .first()
        .cloned()
        .expect("the table carries at least one row");
    for ((case, _, _), answer) in rows.iter().zip(&answers) {
        assert_eq!(
            *answer, first,
            "case {case:?}: the denial must not vary with the resource the request named"
        );
    }

    // The other side: the same ARNs from an entitled caller reach the lookup the denial
    // was ahead of, so the gate's answer was a decision about the ACCOUNT and not a
    // constant.
    for (case, resource_arn, served) in &rows {
        let (status, out) = entitled
            .call("ListTagsForResource", &json!({ "resourceArn": resource_arn }))
            .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: an entitled caller must be served, got {status} {out:?}"
            );
            assert!(
                out.get("tags").and_then(Value::as_array).is_some(),
                "case {case:?}: expected a tags list, got {out:?}"
            );
        } else {
            assert_eq!(
                status,
                served_status("ResourceNotFoundException"),
                "case {case:?}: an entitled caller must get the lookup's 404, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {out:?}"
            );
        }
    }
}
