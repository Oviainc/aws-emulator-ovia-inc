// SPDX-License-Identifier: CC0-1.0

//! `DisassociateBenefitApplicationResource`, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication`, so what these tests pin is the emulator's HTTP behaviour on
//! the persisted record.
//!
//! The rule under test: a disassociation drops exactly the `ResourceArn` it names from the
//! stored application's `AssociatedResources`, keeps every other association, and advances
//! the application's revision and `UpdatedAt` only when the record actually changed — so a
//! resource the application does not carry is a success that changes nothing, and the call
//! is idempotent. The application must exist (404 `ResourceNotFoundException`) and must be
//! one AWS has not finished with (409 `ConflictException` from a terminal status), and
//! nothing is persisted on either refusal.
//!
//! The tables' rows differ along what the rule generalises over: how many resources the
//! application carries, which of them is named (first, middle, last, a duplicate, one it
//! never carried), which ARN form the resource has, which identifier form names the
//! application, which catalog it lives in, and which of the six modelled statuses it is in.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::BENEFIT_APPLICATION_STATUS_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    SEEDED_CATALOGS, TERMINAL_STATUSES,
};

use crate::application_status_gates::reach;
use crate::benefit_application_lifecycle::{error_code, instant, text, Harness, CATALOG};

/// ARNs spanning the forms the modelled `Arn` `@pattern` admits — the shape `ResourceArn`
/// targets, shared with `tests/all/associate_benefit_application_resource.rs`, which
/// states the mirror rule over the same resource forms: `^arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)$`
/// (`smithy_query(query="operation",operation="DisassociateBenefitApplicationResource")`):
/// a service name with a digit and a hyphen, a region and no region, an account and no
/// account, a `-gov` region, and both the `type/id` and the `type:id` resource forms.
pub(crate) const RESOURCE_ARNS: [&str; 6] = [
    "arn:aws:s3:::partner-proposals",
    "arn:aws:ec2:us-west-2:000000000000:instance/i-0123456789abcdef0",
    "arn:aws:iam::000000000000:role/PartnerDelivery",
    "arn:aws:lambda:eu-west-1:000000000000:function:benefit-helper",
    "arn:aws:s3:us-gov-west-1:000000000000:accesspoint/partner-gov",
    "arn:aws:route53-recovery-readiness:us-east-1::cell/partner-cell",
];

/// An application id of the modelled `BenefitApplicationId` form that no store holds.
pub(crate) const UNKNOWN_APPLICATION: &str = "benappl-00000000000001";

/// Create one application in `catalog` carrying `resources`, and return its id.
///
/// `CreateBenefitApplication` is the only operation in this slice that can put
/// associations on a NEW record (`UpdateBenefitApplication` binds no `AssociatedResources`
/// member, and `AssociateBenefitApplicationResource` adds one association at a time to a
/// record that already exists), so every row below starts from a create.
///
/// Shared with `tests/all/associate_benefit_application_resource.rs`, so both suites drive
/// the same seeding path.
pub(crate) async fn seed(
    harness: &Harness,
    case: &str,
    catalog: &str,
    token: &str,
    resources: &[&str],
) -> String {
    let benefit_id = text(case, &harness.active_benefit_of(catalog).await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": catalog,
                "ClientToken": token,
                "BenefitIdentifier": benefit_id,
                "Name": "disassociate-subject",
                "Description": "the application the disassociation rule is asserted on",
                "AssociatedResources": resources,
            }),
        )
        .await;
    text(case, &created, "Id")
}

/// The `AssociatedResources` a response reports, in the order the service reported them.
///
/// A record carrying none omits the member (it is a list the create never stored), which
/// is the empty vector here.
pub(crate) fn associated(body: &Value) -> Vec<String> {
    body["AssociatedResources"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One `DisassociateBenefitApplicationResource` request body — the same three members
/// `AssociateBenefitApplicationResource` binds, so the sibling suite builds its requests
/// with this too.
pub(crate) fn body(catalog: &str, identifier: &str, resource_arn: &str) -> Value {
    json!({
        "Catalog": catalog,
        "BenefitApplicationIdentifier": identifier,
        "ResourceArn": resource_arn,
    })
}

/// A disassociation removes exactly the named resource, leaves the rest of the record
/// alone, and moves the revision and `UpdatedAt` only when the association set changed.
///
/// The rows span the position of the named resource in the set, its ARN form, a set it is
/// not in, an empty set, a duplicated association, and an ARN that differs from a stored
/// one only in its region — and each row's call is repeated, so idempotence is asserted on
/// every one of them.
#[tokio::test]
async fn a_disassociation_removes_exactly_the_named_resource() {
    let harness = Harness::new();
    let [bucket, instance, role, function, access_point, cell] = RESOURCE_ARNS;

    // (case, the associations the create stores, the ARN disassociated, what must remain)
    let rows: Vec<(&str, Vec<&str>, &str, Vec<&str>)> = vec![
        ("the only association", vec![bucket], bucket, vec![]),
        (
            "the first of three",
            vec![bucket, instance, role],
            bucket,
            vec![instance, role],
        ),
        (
            "the middle of three",
            vec![bucket, instance, role],
            instance,
            vec![bucket, role],
        ),
        (
            "the last of three",
            vec![bucket, instance, role],
            role,
            vec![bucket, instance],
        ),
        (
            "one of every ARN form the pattern admits",
            RESOURCE_ARNS.to_vec(),
            access_point,
            vec![bucket, instance, role, function, cell],
        ),
        (
            "a resource the application does not carry",
            vec![bucket, instance],
            function,
            vec![bucket, instance],
        ),
        ("an application carrying no associations", vec![], cell, vec![]),
        (
            "a duplicated association",
            vec![bucket, instance, bucket],
            bucket,
            vec![instance],
        ),
        (
            // Exact match only: an ARN naming the same resource in another region is
            // another resource.
            "an ARN differing from a stored one only in its region",
            vec![instance],
            "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
            vec![instance],
        ),
    ];

    for (row, (case, resources, removed, remaining)) in rows.iter().enumerate() {
        let id = seed(
            &harness,
            case,
            CATALOG,
            &format!("tok-disassociate-{row}"),
            resources,
        )
        .await;
        let before = harness.get_in(case, CATALOG, &id).await;
        let changed = associated(&before) != *remaining;

        let out = harness
            .ok(
                case,
                "DisassociateBenefitApplicationResource",
                &body(CATALOG, &id, removed),
            )
            .await;
        assert_eq!(
            text(case, &out, "Id"),
            id,
            "case {case:?}: expected the application's own id, got {out:?}"
        );
        assert_eq!(
            text(case, &out, "Arn"),
            text(case, &before, "Arn"),
            "case {case:?}: expected the application's stored ARN, got {out:?}"
        );

        let after = harness.get_in(case, CATALOG, &id).await;
        assert_eq!(
            associated(&after),
            *remaining,
            "case {case:?}: expected the associations {remaining:?}, got {after:?}"
        );
        assert_eq!(
            text(case, &out, "Revision"),
            text(case, &after, "Revision"),
            "case {case:?}: the response's revision must be the stored one, got {out:?}"
        );
        let expected_revision = if changed { "2" } else { "1" };
        assert_eq!(
            text(case, &after, "Revision"),
            expected_revision,
            "case {case:?}: expected revision {expected_revision}, got {after:?}"
        );
        if changed {
            assert!(
                instant(case, &after, "UpdatedAt") > instant(case, &after, "CreatedAt"),
                "case {case:?}: a removal must advance UpdatedAt, got {after:?}"
            );
        } else {
            assert_eq!(
                instant(case, &after, "UpdatedAt"),
                instant(case, &before, "UpdatedAt"),
                "case {case:?}: a removal of nothing must leave UpdatedAt, got {after:?}"
            );
        }
        // Nothing else about the record moved.
        for member in ["Id", "Arn", "Catalog", "BenefitId", "Name", "Description", "Status"] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: {member} must not change, got {after:?}"
            );
        }

        // Repeating the same call removes nothing a second time and reports the same
        // revision: the removal is idempotent.
        let repeat = harness
            .ok(
                case,
                "DisassociateBenefitApplicationResource",
                &body(CATALOG, &id, removed),
            )
            .await;
        assert_eq!(
            text(case, &repeat, "Revision"),
            text(case, &after, "Revision"),
            "case {case:?}: a repeated disassociation must not move the revision, got {repeat:?}"
        );
        let twice = harness.get_in(case, CATALOG, &id).await;
        assert_eq!(
            twice, after,
            "case {case:?}: a repeated disassociation must change nothing, got {twice:?}"
        );
    }
}

/// The application may be named by either modelled `BenefitApplicationIdentifier` form —
/// its id or its ARN — in either seeded catalog, and the answer names the application's
/// own id and ARN whichever form was sent.
#[tokio::test]
async fn a_disassociation_accepts_every_identifier_form_in_every_catalog() {
    let harness = Harness::new();
    let [bucket, instance, ..] = RESOURCE_ARNS;

    let mut row = 0usize;
    for catalog in SEEDED_CATALOGS {
        for form in ["the id", "the ARN"] {
            let case = format!("{form} in {catalog}");
            let id = seed(
                &harness,
                &case,
                catalog,
                &format!("tok-disassociate-form-{row}"),
                &[bucket, instance],
            )
            .await;
            row += 1;
            let created = harness.get_in(&case, catalog, &id).await;
            let arn = text(&case, &created, "Arn");
            let identifier = if form == "the id" { id.clone() } else { arn.clone() };

            let out = harness
                .ok(
                    &case,
                    "DisassociateBenefitApplicationResource",
                    &body(catalog, &identifier, instance),
                )
                .await;
            assert_eq!(
                text(&case, &out, "Id"),
                id,
                "case {case:?}: expected the application's id, got {out:?}"
            );
            assert_eq!(
                text(&case, &out, "Arn"),
                arn,
                "case {case:?}: expected the application's ARN, got {out:?}"
            );

            let after = harness.get_in(&case, catalog, &id).await;
            assert_eq!(
                associated(&after),
                vec![bucket.to_string()],
                "case {case:?}: expected only the resource that was not named, got {after:?}"
            );
            assert_eq!(
                text(&case, &after, "Revision"),
                "2",
                "case {case:?}: an accepted removal must bump the revision, got {after:?}"
            );
        }
    }
}

/// A disassociation naming no stored application is refused with a 404
/// `ResourceNotFoundException`.
///
/// The rows span the ways the lookup can fail: an id of the modelled form that nothing
/// holds, that same id's ARN, the ARN of an application of another catalog, and a stored
/// application named through a catalog the service does not serve.
#[tokio::test]
async fn a_disassociation_of_an_application_that_does_not_exist_is_a_404() {
    let harness = Harness::new();
    let [bucket, ..] = RESOURCE_ARNS;

    let id = seed(
        &harness,
        "the stored application",
        CATALOG,
        "tok-disassociate-404",
        &[bucket],
    )
    .await;
    let stored = harness.get_in("the stored application", CATALOG, &id).await;
    let arn = text("the stored application", &stored, "Arn");
    let other_catalog = SEEDED_CATALOGS
        .iter()
        .find(|catalog| **catalog != CATALOG)
        .expect("the service seeds more than one catalog");

    let rows: Vec<(String, &str, String)> = vec![
        (
            "an id nothing holds".to_string(),
            CATALOG,
            UNKNOWN_APPLICATION.to_string(),
        ),
        (
            "the ARN of an id nothing holds".to_string(),
            CATALOG,
            arn.replace(&id, UNKNOWN_APPLICATION),
        ),
        (
            "a stored application named through another catalog".to_string(),
            other_catalog,
            id.clone(),
        ),
        (
            "a stored application named through a catalog the service does not serve"
                .to_string(),
            "no-such-catalog",
            id.clone(),
        ),
        (
            "the stored ARN named through another catalog".to_string(),
            other_catalog,
            arn.clone(),
        ),
    ];

    for (case, catalog, identifier) in &rows {
        let (http, out) = harness
            .call(
                "DisassociateBenefitApplicationResource",
                &body(catalog, identifier, bucket),
            )
            .await;
        assert_eq!(
            http, 404,
            "case {case:?}: expected 404, got {http} with {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {out:?}"
        );
    }

    // And the stored application kept its association through every refusal.
    let after = harness.get_in("the stored application", CATALOG, &id).await;
    assert_eq!(
        after, stored,
        "case \"the stored application\": a refused disassociation must persist nothing, got {after:?}"
    );
}

/// A disassociation is accepted while AWS has not finished with the application, and
/// refused with a 409 `ConflictException` once its status is terminal — with nothing
/// persisted.
///
/// The rows are every modelled `BenefitApplicationStatus`
/// (`BENEFIT_APPLICATION_STATUS_VALUES`, so the set cannot drift from the model), each
/// naming a different ARN form so the gate is not specific to one resource.
#[tokio::test]
async fn a_disassociation_is_refused_once_the_application_is_terminal() {
    let harness = Harness::new();

    for (row, status) in BENEFIT_APPLICATION_STATUS_VALUES.iter().enumerate() {
        let removed = RESOURCE_ARNS[row % RESOURCE_ARNS.len()];
        let kept = RESOURCE_ARNS[(row + 1) % RESOURCE_ARNS.len()];
        let case = format!("{status}, disassociating {removed}");
        let id = seed(
            &harness,
            &case,
            CATALOG,
            &format!("tok-disassociate-gate-{row}"),
            &[removed, kept],
        )
        .await;
        reach(&harness, &case, CATALOG, &id, status).await;
        let before = harness.get_in(&case, CATALOG, &id).await;

        if TERMINAL_STATUSES.contains(status) {
            let (http, out) = harness
                .call(
                    "DisassociateBenefitApplicationResource",
                    &body(CATALOG, &id, removed),
                )
                .await;
            assert_eq!(
                http, 409,
                "case {case:?}: expected 409 from a {status} application, got {http} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {out:?}"
            );
            let after = harness.get_in(&case, CATALOG, &id).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused disassociation must persist nothing, got {after:?}"
            );
            continue;
        }

        // Not terminal: managing the application's resources is what this operation is
        // for, so the removal applies and the read reports it, without moving the status.
        harness
            .ok(
                &case,
                "DisassociateBenefitApplicationResource",
                &body(CATALOG, &id, removed),
            )
            .await;
        let after = harness.get_in(&case, CATALOG, &id).await;
        assert_eq!(
            associated(&after),
            vec![kept.to_string()],
            "case {case:?}: expected only the resource that was not named, got {after:?}"
        );
        assert_ne!(
            text(&case, &after, "Revision"),
            text(&case, &before, "Revision"),
            "case {case:?}: an accepted removal must bump the revision, got {after:?}"
        );
        assert_eq!(
            after["Status"],
            json!(status),
            "case {case:?}: a disassociation must not move the status, got {after:?}"
        );
    }
}

/// The removal is observable through the list filter too: an application is no longer
/// reported for the resource it was disassociated from, and is still reported for the one
/// it kept.
#[tokio::test]
async fn a_disassociated_resource_no_longer_matches_the_list_filter() {
    let harness = Harness::new();
    let [bucket, instance, ..] = RESOURCE_ARNS;
    let case = "the list filter after a disassociation";

    let id = seed(
        &harness,
        case,
        CATALOG,
        "tok-disassociate-filter",
        &[bucket, instance],
    )
    .await;

    let matching = |out: &Value| -> bool {
        out["BenefitApplicationSummaries"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .any(|summary| summary["Id"] == json!(id))
    };

    for arn in [bucket, instance] {
        let out = harness
            .ok(
                case,
                "ListBenefitApplications",
                &json!({ "Catalog": CATALOG, "AssociatedResourceArns": [arn] }),
            )
            .await;
        assert!(
            matching(&out),
            "case {case:?}: the created application must match the filter {arn:?}, got {out:?}"
        );
    }

    harness
        .ok(
            case,
            "DisassociateBenefitApplicationResource",
            &body(CATALOG, &id, instance),
        )
        .await;

    let gone = harness
        .ok(
            case,
            "ListBenefitApplications",
            &json!({ "Catalog": CATALOG, "AssociatedResourceArns": [instance] }),
        )
        .await;
    assert!(
        !matching(&gone),
        "case {case:?}: the application must no longer match the disassociated resource, got {gone:?}"
    );
    let kept = harness
        .ok(
            case,
            "ListBenefitApplications",
            &json!({ "Catalog": CATALOG, "AssociatedResourceArns": [bucket] }),
        )
        .await;
    assert!(
        matching(&kept),
        "case {case:?}: the application must still match the resource it kept, got {kept:?}"
    );
}
