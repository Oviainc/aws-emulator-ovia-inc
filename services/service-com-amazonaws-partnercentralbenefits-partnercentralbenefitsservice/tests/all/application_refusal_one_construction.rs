// SPDX-License-Identifier: CC0-1.0

//! The single construction of the application-not-found refusal, driven end to end over
//! the wire through one call per former hand-rolled copy.
//!
//! The rule under test: every operation that resolves a benefit application answers a
//! request naming one this service does not hold with the ONE refusal
//! `ComAmazonawsPartnercentralbenefitsPartnercentralbenefitsserviceState::require_application`
//! (and its writing counterpart `require_application_mut`) builds, so the wire answer of
//! every such operation moves with that helper and none of them can carry its own resource
//! kind or quote an identifier other than the one the request named.
//!
//! The table spans the class rather than one call: all eight operations that resolve an
//! application — the reporting one (`GetBenefitApplication`) and the seven writing ones
//! (`SubmitBenefitApplication`, `UpdateBenefitApplication`, `CancelBenefitApplication`,
//! `AmendBenefitApplication`, `RecallBenefitApplication`,
//! `AssociateBenefitApplicationResource`, `DisassociateBenefitApplicationResource`) —
//! crossed with identifiers of both forms the modelled `BenefitApplicationIdentifier`
//! admits (`^(arn:.+|benappl-[0-9a-z]{14})$`,
//! `smithy_query(query="shape",shape="BenefitApplicationIdentifier")`), and — on the other
//! side of the rule — with an application this store does hold.
//!
//! Each expected message is taken from the helper itself, read out of the live store, so a
//! change to what the helper builds changes what every row must answer.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    service_state, APPLICATION_RESOURCE,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, ACCOUNT, CATALOG, REGION};
use crate::service_error_rules::message_of;

/// An AWS resource ARN the modelled `ResourceArn` pattern admits, for the two operations
/// that bind one (`^arn:aws:([a-zA-Z0-9\-])+:([a-z]{2}(-gov)?-[a-z]+-\d{1})?:(\d{12})?:(.+)$`).
const RESOURCE_ARN: &str = "arn:aws:ec2:us-west-2:000000000000:instance/i-0123456789abcdef0";

/// The refusal the state helper builds for `identifier` in this harness's own store: the
/// one construction every row below must answer with, and `None` when the application
/// exists.
fn helper_refusal(harness: &Harness, catalog: &str, identifier: &str) -> Option<String> {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .require_application(catalog, identifier)
        .err()
        .map(|refusal| refusal.message)
}

/// One well-formed request per operation in the table, each naming `identifier` through
/// whichever member that operation binds it with.
///
/// These are exactly the eight operations that resolve an application, so the table is the
/// whole class the single construction serves. `token_suffix` keeps every `ClientToken`
/// distinct across rows — a repeated token would make the next row a retry of the previous
/// request rather than a new one — and every token here is admitted by the narrowest
/// modelled pattern on one (`^[a-zA-Z0-9-]{1,64}$`, on the cancel and the amend).
fn application_bound_requests(
    identifier: &str,
    revision: &str,
    token_suffix: &str,
) -> Vec<(&'static str, Value)> {
    vec![
        (
            "GetBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": identifier }),
        ),
        (
            "SubmitBenefitApplication",
            json!({ "Catalog": CATALOG, "Identifier": identifier }),
        ),
        (
            "UpdateBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": format!("appl-refusal-update-{token_suffix}"),
                "Identifier": identifier,
                "Revision": revision,
                "Name": "an application-refusal probe",
            }),
        ),
        (
            "CancelBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": format!("appl-refusal-cancel-{token_suffix}"),
                "Identifier": identifier,
            }),
        ),
        (
            "AmendBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "ClientToken": format!("appl-refusal-amend-{token_suffix}"),
                "Identifier": identifier,
                "Revision": revision,
                "AmendmentReason": "the partner corrected the application",
                "Amendments": [{ "FieldPath": "Name", "NewValue": "amended" }],
            }),
        ),
        (
            "RecallBenefitApplication",
            json!({
                "Catalog": CATALOG,
                "Identifier": identifier,
                "Reason": "the partner withdrew the application",
            }),
        ),
        (
            "AssociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": identifier,
                "ResourceArn": RESOURCE_ARN,
            }),
        ),
        (
            "DisassociateBenefitApplicationResource",
            json!({
                "Catalog": CATALOG,
                "BenefitApplicationIdentifier": identifier,
                "ResourceArn": RESOURCE_ARN,
            }),
        ),
    ]
}

/// One identifier spelled as an ARN of this service, written out here rather than built
/// through the service's own helper precisely because a row carries fields the service
/// does not mint.
fn application_arn(region: &str, account: &str, catalog: &str, id: &str) -> String {
    let resource = APPLICATION_RESOURCE;
    format!("arn:aws:partnercentral:{region}:{account}:catalog/{catalog}/{resource}/{id}")
}

/// Identifiers no create ever minted, each admitted by the modelled
/// `BenefitApplicationIdentifier` pattern: four bare ids spanning the `[0-9a-z]{14}` body,
/// then five ARN spellings — the one this service would issue for an unminted id, the same
/// in another region, under another account and in another catalog, and an ARN of another
/// service entirely (`arn:.+` admits it).
fn unknown_identifiers() -> Vec<(&'static str, String)> {
    let unminted = "benappl-00000000000001";
    vec![
        ("an all-zero id", "benappl-00000000000000".to_string()),
        ("digits only", unminted.to_string()),
        ("letters only", "benappl-zzzzzzzzzzzzzz".to_string()),
        (
            "mixed digits and letters",
            "benappl-9a8b7c6d5e4f3g".to_string(),
        ),
        (
            "the ARN this service would issue",
            application_arn(REGION, ACCOUNT, CATALOG, unminted),
        ),
        (
            "an ARN of another region",
            application_arn("eu-west-1", ACCOUNT, CATALOG, unminted),
        ),
        (
            "an ARN of another account",
            application_arn(REGION, "123456789012", CATALOG, unminted),
        ),
        (
            "an ARN of another catalog",
            application_arn(REGION, ACCOUNT, "Sandbox", unminted),
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:::partner-proposals".to_string(),
        ),
    ]
}

/// Every operation that resolves an application answers an identifier this store does not
/// hold with exactly the refusal the state helper builds — the eight former inline copies
/// included.
#[tokio::test]
async fn every_operation_answers_the_refusal_the_one_helper_builds() {
    let harness = Harness::new();
    for (index, (case, identifier)) in unknown_identifiers().into_iter().enumerate() {
        let expected = helper_refusal(&harness, CATALOG, &identifier).unwrap_or_else(|| {
            panic!("case {case:?}: the helper must refuse the unknown identifier {identifier:?}")
        });
        // The revision is never reached on these rows — the lookup refuses first — so any
        // value the modelled `Revision` string admits serves.
        let requests = application_bound_requests(&identifier, "1", &index.to_string());
        for (operation, body) in &requests {
            let (status, response) = harness.call(operation, body).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected 404, got {status} with {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got \
                 {response:?}"
            );
            let message = message_of(&response).unwrap_or_else(|| {
                panic!("case {case:?} on {operation}: the answer carries no message: {response:?}")
            });
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected the helper's refusal {expected:?}, got \
                 {message:?}"
            );
            assert!(
                message.contains(&format!("`{identifier}`")),
                "case {case:?} on {operation}: the message must quote the identifier the \
                 request named, got {message:?}"
            );
        }
    }
}

/// The other side of the rule: for an application this store holds the helper builds no
/// refusal, and every operation in the table gets past the lookup and answers 200.
///
/// Each row gets its own freshly created draft, so one row's write cannot decide the next
/// row's answer.
#[tokio::test]
async fn an_application_the_store_holds_is_refused_by_no_operation() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit_of(CATALOG).await, "Id");
    // The table's own operation list, so the two tests cannot span different sets.
    let operations: Vec<&'static str> =
        application_bound_requests("benappl-00000000000000", "1", "shape")
            .into_iter()
            .map(|(operation, _)| operation)
            .collect();

    for (index, operation) in operations.into_iter().enumerate() {
        let case = format!("{operation} on a stored application");
        let created = harness
            .create(
                &case,
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("appl-refusal-seed-{index}"),
                    "BenefitIdentifier": benefit_id,
                    "Name": "an application the single-construction rule is asserted on",
                }),
            )
            .await;
        let id = text(&case, &created, "Id");
        let revision = text(&case, &created, "Revision");
        let refusal = helper_refusal(&harness, CATALOG, &id);
        assert!(
            refusal.is_none(),
            "case {case:?}: the helper must not refuse an application this store holds, got \
             {refusal:?}"
        );

        // `RecallBenefitApplication` returns a SUBMITTED application to draft, so this
        // row's draft is submitted first; the lookup under test is the same either way.
        // The recall binds no `Revision`, so the submit's revision bump is immaterial here.
        if operation == "RecallBenefitApplication" {
            harness
                .ok(
                    &case,
                    "SubmitBenefitApplication",
                    &json!({ "Catalog": CATALOG, "Identifier": id }),
                )
                .await;
        }

        let body = application_bound_requests(&id, &revision, &format!("held-{index}"))
            .into_iter()
            .find(|(name, _)| *name == operation)
            .map(|(_, body)| body)
            .unwrap_or_else(|| panic!("case {case:?}: the table must hold {operation}"));
        let (status, response) = harness.call(operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response:?}"
        );
    }
}
