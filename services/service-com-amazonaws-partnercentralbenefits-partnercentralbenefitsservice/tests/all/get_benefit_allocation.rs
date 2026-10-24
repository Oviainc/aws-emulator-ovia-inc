// SPDX-License-Identifier: CC0-1.0

//! `GetBenefitAllocation`, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rules under test, each stated over a CLASS of inputs and checked as a table whose
//! rows differ along what the rule generalises over:
//!
//! 1. **The report is the store**: the operation reports the one benefit allocation the
//!    request names, member for member from the stored record, and the ARN it reports is
//!    the allocation ARN of the region the request was signed for. Rows: every stored
//!    allocation of every seeded catalog, plus an allocation AWS granted for an
//!    application.
//! 2. **Either identifier form names the same record**: the bare `benalloc-...` id and an
//!    allocation ARN of that catalog are answered identically. Rows: every stored
//!    allocation of every seeded catalog, in both forms.
//! 3. **`FulfillmentDetail` is the member of the modelled `FulfillmentDetails` union the
//!    allocation's `FulfillmentType` names** — `CASH` is disbursed, `CREDITS` is issued as
//!    credit codes, `ACCESS` grants a privilege — and every monetary amount it carries is
//!    the stored one, in a modelled `CurrencyCode`. Rows: every `FulfillmentType` the model
//!    declares, an allocation partly issued, one issued in full, and one with nothing
//!    issued yet.
//! 4. **An identifier that names no allocation of the catalog is refused**
//!    `ResourceNotFoundException` (404) with the service's own lookup message. Rows: a
//!    well-formed id no grant ever minted, an allocation ARN of the other catalog, an ARN
//!    of another resource type of the same catalog (a benefit, an application), and an ARN
//!    of another service.
//! 5. **A catalog the service does not serve is refused** `ResourceNotFoundException` (404)
//!    with the catalog-lookup message, whatever the identifier. Rows: two catalogs the
//!    service does not serve, each with a stored id and with an unknown one.
//! 6. **Reading changes nothing a later caller sees, except materialising an expiry the
//!    clock already made due**: an allocation past its `ExpiresAt` is never reported
//!    `ACTIVE` and a credit code past its own is never reported redeemable, while two reads
//!    of any allocation are identical.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::{
    BENEFIT_ALLOCATION_STATUS_VALUES, CURRENCY_CODE_VALUES, FULFILLMENT_TYPE_VALUES,
};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    not_found_message, now_millis, service_state, AllocatedCreditCode, AllocationFulfillment,
    BenefitAllocation, Issuance, Money, ALLOCATION_EXPIRED_REASON, ALLOCATION_RESOURCE,
    ALLOCATION_STATUS_ACTIVE, ALLOCATION_STATUS_FULFILLED, ALLOCATION_STATUS_INACTIVE,
    ALLOCATION_KIND, APPLICATION_RESOURCE, BENEFIT_RESOURCE, CATALOG_KIND,
    FULFILLMENT_TYPE_ACCESS, FULFILLMENT_TYPE_CASH, FULFILLMENT_TYPE_CREDITS, SEEDED_CATALOGS,
};

use crate::benefit_allocations::{granted, stored_allocations};
use crate::benefit_application_lifecycle::{error_code, instant, Harness, CATALOG};
use crate::resource_tagging::{application_arn, benefit_id, error_message, expected_arn};

/// The other catalog the service serves, so a row can name an ARN of a catalog that is not
/// the one the request asked about.
const OTHER_CATALOG: &str = "Sandbox";

/// An allocation id the modelled `BenefitAllocationIdentifier` admits and no grant ever
/// minted, so it names nothing.
const UNKNOWN_ALLOCATION: &str = "benalloc-00000000000000";

/// Which member of the modelled `FulfillmentDetails` union each `FulfillmentType` is
/// fulfilled through.
///
/// From the union's own member docs (`smithy_query(query="shape",shape="FulfillmentDetails"
/// )`): `DisbursementDetails` is "direct payments or reimbursements", `CreditDetails` is
/// "credit-based benefit fulfillment, including AWS promotional credits" and
/// `AccessDetails` is "service permissions or feature access". The union's fourth member,
/// `ConsumableDetails`, is produced by no `FulfillmentType` the model declares.
const DETAIL_MEMBER_BY_TYPE: &[(&str, &str)] = &[
    (FULFILLMENT_TYPE_CASH, "DisbursementDetails"),
    (FULFILLMENT_TYPE_CREDITS, "CreditDetails"),
    (FULFILLMENT_TYPE_ACCESS, "AccessDetails"),
];

/// One `GetBenefitAllocation` call that must answer 200.
async fn get(harness: &Harness, case: &str, catalog: &str, identifier: &str) -> Value {
    harness
        .ok(
            case,
            "GetBenefitAllocation",
            &json!({ "Catalog": catalog, "Identifier": identifier }),
        )
        .await
}

/// One `GetBenefitAllocation` call that must be refused with `ResourceNotFoundException`
/// (404) carrying `expected_message`.
async fn refused(
    harness: &Harness,
    case: &str,
    catalog: &str,
    identifier: &str,
    expected_message: &str,
) {
    let (status, out) = harness
        .call(
            "GetBenefitAllocation",
            &json!({ "Catalog": catalog, "Identifier": identifier }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} with {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {out:?}"
    );
    assert_eq!(
        error_message(&out),
        expected_message,
        "case {case:?}: expected the service's lookup message, got {out:?}"
    );
}

/// A string member of a response, or `None` when it omits it.
fn member(body: &Value, name: &str) -> Option<String> {
    body.get(name).and_then(Value::as_str).map(str::to_string)
}

/// The entries of a list member, or an empty list when the response omits it.
fn members(body: &Value, name: &str) -> Vec<String> {
    body.get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Assert one reported `MonetaryValue` is the stored amount: the amount text the store
/// renders, and a currency the model declares.
fn assert_money(case: &str, label: &str, reported: &Value, expected: &Money) {
    let amount = member(reported, "Amount");
    assert_eq!(
        amount.as_deref(),
        Some(expected.amount_text().as_str()),
        "case {case:?}: expected {label} Amount {:?}, got {reported:?}",
        expected.amount_text()
    );
    let currency = member(reported, "CurrencyCode");
    assert_eq!(
        currency.as_deref(),
        Some(expected.currency_code.as_str()),
        "case {case:?}: expected {label} CurrencyCode {:?}, got {reported:?}",
        expected.currency_code
    );
    let currency = currency.unwrap_or_default();
    assert!(
        CURRENCY_CODE_VALUES.contains(&currency.as_str()),
        "case {case:?}: {label} CurrencyCode {currency:?} is not a modelled CurrencyCode"
    );
    // A two-decimal amount, which is what a minor unit of this currency renders as.
    let amount = amount.unwrap_or_default();
    let (units, cents) = amount
        .split_once('.')
        .unwrap_or_else(|| panic!("case {case:?}: {label} Amount {amount:?} carries no decimals"));
    assert!(
        !units.is_empty()
            && units.bytes().all(|b| b.is_ascii_digit())
            && cents.len() == 2
            && cents.bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: expected {label} Amount as digits with two decimals, got {amount:?}"
    );
}

/// Assert one reported `IssuanceDetail` is the stored issuance.
fn assert_issuance(case: &str, reported: &Value, expected: &Issuance) {
    assert_eq!(
        member(reported, "IssuanceId").as_deref(),
        Some(expected.id.as_str()),
        "case {case:?}: expected the stored IssuanceId, got {reported:?}"
    );
    assert_money(
        case,
        "IssuanceAmount",
        &reported["IssuanceAmount"],
        &expected.amount,
    );
    assert_eq!(
        instant(case, reported, "IssuedAt"),
        expected.issued_at_millis,
        "case {case:?}: expected the stored IssuedAt, got {reported:?}"
    );
}

/// Assert one reported `CreditCode` is the stored credit code.
fn assert_credit_code(case: &str, reported: &Value, expected: &AllocatedCreditCode) {
    assert_eq!(
        member(reported, "AwsAccountId").as_deref(),
        Some(expected.aws_account_id.as_str()),
        "case {case:?}: expected the stored AwsAccountId, got {reported:?}"
    );
    assert_eq!(
        member(reported, "AwsCreditCode").as_deref(),
        Some(expected.code.as_str()),
        "case {case:?}: expected the stored AwsCreditCode, got {reported:?}"
    );
    assert_money(case, "CreditCode Value", &reported["Value"], &expected.value);
    let status = member(reported, "Status").unwrap_or_default();
    assert_eq!(
        status, expected.status,
        "case {case:?}: expected the stored credit-code Status, got {reported:?}"
    );
    assert!(
        BENEFIT_ALLOCATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: credit-code Status {status:?} is not a modelled BenefitAllocationStatus"
    );
    assert_eq!(
        instant(case, reported, "IssuedAt"),
        expected.issued_at_millis,
        "case {case:?}: expected the stored credit-code IssuedAt, got {reported:?}"
    );
    assert_eq!(
        instant(case, reported, "ExpiresAt"),
        expected.expires_at_millis,
        "case {case:?}: expected the stored credit-code ExpiresAt, got {reported:?}"
    );
}

/// Assert the reported `FulfillmentDetail` is the union member the allocation's stored
/// fulfillment record names, carrying that record's own values — and that it carries
/// exactly ONE member, since the modelled `FulfillmentDetails` is a union.
fn assert_fulfillment_detail(case: &str, body: &Value, allocation: &BenefitAllocation) {
    let reported = &body["FulfillmentDetail"];
    let Some(fulfillment) = allocation.fulfillment.as_ref() else {
        assert!(
            reported.is_null(),
            "case {case:?}: an allocation with no stored fulfillment record reports no \
             FulfillmentDetail, got {reported:?}"
        );
        return;
    };
    let expected_member = DETAIL_MEMBER_BY_TYPE
        .iter()
        .find(|(fulfillment_type, _)| *fulfillment_type == allocation.fulfillment_type)
        .map(|(_, detail_member)| *detail_member)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {:?} names no FulfillmentDetails member",
                allocation.fulfillment_type
            )
        });
    let object = reported
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a FulfillmentDetail, got {reported:?}"));
    assert_eq!(
        object.keys().collect::<Vec<&String>>(),
        vec![expected_member],
        "case {case:?}: expected exactly the union member {expected_member:?}, got {reported:?}"
    );

    let detail = &reported[expected_member];
    match fulfillment {
        AllocationFulfillment::Disbursement {
            disbursed,
            issuance,
        } => {
            assert_money(case, "DisbursedAmount", &detail["DisbursedAmount"], disbursed);
            match issuance {
                Some(issuance) => assert_issuance(case, &detail["IssuanceDetails"], issuance),
                None => assert!(
                    detail["IssuanceDetails"].is_null(),
                    "case {case:?}: nothing disbursed yet reports no IssuanceDetails, got {detail:?}"
                ),
            }
        }
        AllocationFulfillment::Credits {
            allocated,
            issued,
            codes,
        } => {
            assert_money(case, "AllocatedAmount", &detail["AllocatedAmount"], allocated);
            assert_money(case, "IssuedAmount", &detail["IssuedAmount"], issued);
            let reported_codes = detail["Codes"].as_array().unwrap_or_else(|| {
                panic!("case {case:?}: CreditDetails$Codes is @required, got {detail:?}")
            });
            assert_eq!(
                reported_codes.len(),
                codes.len(),
                "case {case:?}: expected {} credit codes, got {detail:?}",
                codes.len()
            );
            for (index, (reported_code, expected_code)) in
                reported_codes.iter().zip(codes).enumerate()
            {
                assert_credit_code(&format!("{case}/code {index}"), reported_code, expected_code);
            }
        }
        AllocationFulfillment::Access => assert_eq!(
            member(detail, "Description"),
            allocation.description.clone(),
            "case {case:?}: expected the allocation's own Description, got {detail:?}"
        ),
    }
}

/// Assert one reported allocation is the stored record, member for member.
fn assert_reports_the_store(case: &str, body: &Value, allocation: &BenefitAllocation) {
    assert_eq!(
        member(body, "Id").as_deref(),
        Some(allocation.id.as_str()),
        "case {case:?}: expected the stored Id, got {body:?}"
    );
    assert_eq!(
        member(body, "Catalog").as_deref(),
        Some(allocation.catalog.as_str()),
        "case {case:?}: expected the stored Catalog, got {body:?}"
    );
    assert_eq!(
        member(body, "Arn"),
        Some(expected_arn(
            &allocation.catalog,
            ALLOCATION_RESOURCE,
            &allocation.id
        )),
        "case {case:?}: expected the allocation ARN of the signed region, got {body:?}"
    );
    assert_eq!(
        member(body, "Name").as_deref(),
        Some(allocation.name.as_str()),
        "case {case:?}: expected the stored Name, got {body:?}"
    );
    assert_eq!(
        member(body, "Description"),
        allocation.description.clone(),
        "case {case:?}: expected the stored Description, got {body:?}"
    );
    let status = member(body, "Status").unwrap_or_default();
    assert_eq!(
        status, allocation.status,
        "case {case:?}: expected the stored Status, got {body:?}"
    );
    assert!(
        BENEFIT_ALLOCATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: Status {status:?} is not a modelled BenefitAllocationStatus"
    );
    assert_eq!(
        member(body, "StatusReason"),
        allocation.status_reason.clone(),
        "case {case:?}: expected the stored StatusReason, got {body:?}"
    );
    assert_eq!(
        member(body, "BenefitId").as_deref(),
        Some(allocation.benefit_id.as_str()),
        "case {case:?}: expected the stored BenefitId, got {body:?}"
    );
    assert_eq!(
        member(body, "BenefitApplicationId"),
        allocation.benefit_application_id.clone(),
        "case {case:?}: expected the stored BenefitApplicationId, got {body:?}"
    );
    let fulfillment_type = member(body, "FulfillmentType").unwrap_or_default();
    assert_eq!(
        fulfillment_type, allocation.fulfillment_type,
        "case {case:?}: expected the stored FulfillmentType, got {body:?}"
    );
    assert!(
        FULFILLMENT_TYPE_VALUES.contains(&fulfillment_type.as_str()),
        "case {case:?}: FulfillmentType {fulfillment_type:?} is not a modelled FulfillmentType"
    );
    assert!(
        allocation.fulfillment_types.contains(&fulfillment_type),
        "case {case:?}: an allocation is fulfilled with one of its benefit's types {:?}, \
         got {fulfillment_type:?}",
        allocation.fulfillment_types
    );
    assert_eq!(
        members(body, "ApplicableBenefitIds"),
        allocation.applicable_benefit_ids,
        "case {case:?}: expected the stored ApplicableBenefitIds, got {body:?}"
    );
    for (reported, stored) in [
        ("CreatedAt", allocation.created_at_millis),
        ("UpdatedAt", allocation.updated_at_millis),
        ("StartsAt", allocation.starts_at_millis),
        ("ExpiresAt", allocation.expires_at_millis),
    ] {
        assert_eq!(
            instant(case, body, reported),
            stored,
            "case {case:?}: expected the stored {reported}, got {body:?}"
        );
    }
    assert_fulfillment_detail(case, body, allocation);
}

/// Grant one allocation of `CATALOG` as the outcome of a real benefit application, so the
/// tables below cover a record AWS's fulfillment workflow produced as well as the seeded
/// ones. Returns its id.
async fn granted_for_an_application(harness: &Harness, token: &str) -> String {
    let application = application_arn(harness, token, CATALOG, token, &[]).await;
    let benefit = benefit_id(harness, token, CATALOG).await;
    granted(harness, CATALOG, &benefit, Some(&application), token)
}

/// Rule 1: every stored allocation of every seeded catalog, and one AWS granted for an
/// application, is reported member for member from the record.
#[tokio::test]
async fn reports_the_stored_allocation_member_for_member_in_every_catalog() {
    let harness = Harness::new();
    let granted_id = granted_for_an_application(&harness, "tok-get-allocation").await;
    let mut seen = 0usize;

    for catalog in SEEDED_CATALOGS {
        let ids: Vec<String> = stored_allocations(&harness, catalog)
            .into_iter()
            .map(|allocation| allocation.id)
            .collect();
        assert!(
            !ids.is_empty(),
            "case {catalog:?}: the seeded catalog must hold allocations to report"
        );
        // Every read is taken first, so the store is then compared AFTER the expiries rule
        // 6 covers have been materialised and the two sides compare like for like.
        let mut bodies: Vec<(String, Value)> = Vec::new();
        for id in &ids {
            let case = format!("{catalog}/{id}");
            bodies.push((id.clone(), get(&harness, &case, catalog, id).await));
        }
        let stored = stored_allocations(&harness, catalog);
        for (id, body) in &bodies {
            let case = format!("{catalog}/{id}");
            let allocation = stored
                .iter()
                .find(|allocation| &allocation.id == id)
                .unwrap_or_else(|| panic!("case {case:?}: the store lost the allocation"));
            assert_reports_the_store(&case, body, allocation);
            seen += 1;
        }
        if *catalog == CATALOG {
            assert!(
                ids.contains(&granted_id),
                "case {catalog:?}: the granted allocation must be among the stored ones"
            );
        }
    }

    assert!(
        seen >= SEEDED_CATALOGS.len() * 4,
        "the table must cover every seeded allocation of both catalogs, covered {seen}"
    );
}

/// Rule 2: the bare id and the allocation ARN of the catalog are answered identically.
#[tokio::test]
async fn either_identifier_form_names_the_same_allocation() {
    let harness = Harness::new();
    granted_for_an_application(&harness, "tok-get-allocation-forms").await;

    for catalog in SEEDED_CATALOGS {
        for allocation in stored_allocations(&harness, catalog) {
            let case = format!("{catalog}/{}", allocation.id);
            let by_id = get(&harness, &case, catalog, &allocation.id).await;
            let arn = expected_arn(catalog, ALLOCATION_RESOURCE, &allocation.id);
            let by_arn = get(&harness, &case, catalog, &arn).await;
            assert_eq!(
                by_arn, by_id,
                "case {case:?}: the ARN form must be answered exactly as the id form, \
                 got {by_arn:?} and {by_id:?}"
            );
        }
    }
}

/// Rule 3: the `FulfillmentDetail` member is the one the allocation's `FulfillmentType`
/// names, and the table covers every `FulfillmentType` the model declares.
#[tokio::test]
async fn the_fulfillment_detail_is_the_union_member_the_fulfillment_type_names() {
    let harness = Harness::new();
    granted_for_an_application(&harness, "tok-get-allocation-detail").await;
    let mut covered: BTreeSet<String> = BTreeSet::new();
    let mut issued_states: BTreeSet<bool> = BTreeSet::new();

    for catalog in SEEDED_CATALOGS {
        for allocation in stored_allocations(&harness, catalog) {
            let case = format!("{catalog}/{}", allocation.fulfillment_type);
            let body = get(&harness, &case, catalog, &allocation.id).await;
            // `assert_reports_the_store` asserts the same mapping over the whole record;
            // here it is the rule under test, over the store read after the call.
            let stored = stored_allocations(&harness, catalog);
            let allocation = stored
                .iter()
                .find(|stored| stored.id == allocation.id)
                .unwrap_or_else(|| panic!("case {case:?}: the store lost the allocation"));
            assert_fulfillment_detail(&case, &body, allocation);
            covered.insert(allocation.fulfillment_type.clone());
            issued_states.insert(match &allocation.fulfillment {
                Some(AllocationFulfillment::Disbursement { issuance, .. }) => issuance.is_some(),
                Some(AllocationFulfillment::Credits { codes, .. }) => !codes.is_empty(),
                _ => false,
            });
        }
    }

    for fulfillment_type in FULFILLMENT_TYPE_VALUES {
        assert!(
            covered.contains(*fulfillment_type),
            "case {fulfillment_type:?}: the table must cover every modelled FulfillmentType, \
             covered {covered:?}"
        );
    }
    assert_eq!(
        issued_states.len(),
        2,
        "the table must cover an allocation with an issuance and one with none, covered \
         {issued_states:?}"
    );
}

/// Rule 4: an identifier naming no allocation of the catalog is refused with the
/// allocation-lookup message.
#[tokio::test]
async fn an_identifier_that_names_no_allocation_of_the_catalog_is_refused() {
    let harness = Harness::new();
    let application = application_arn(&harness, "tok-get-allocation-404", CATALOG, "tok-404", &[])
        .await;
    let benefit = benefit_id(&harness, "a benefit of the catalog", CATALOG).await;
    let other_catalog_allocation = stored_allocations(&harness, OTHER_CATALOG)
        .first()
        .map(|allocation| allocation.id.clone())
        .expect("the other seeded catalog holds allocations");

    // (case, the identifier the request names)
    let rows: Vec<(&str, String)> = vec![
        (
            "a well-formed id no grant ever minted",
            UNKNOWN_ALLOCATION.to_string(),
        ),
        (
            "an allocation ARN of the other catalog",
            expected_arn(OTHER_CATALOG, ALLOCATION_RESOURCE, &other_catalog_allocation),
        ),
        (
            "the other catalog's allocation id",
            other_catalog_allocation.clone(),
        ),
        (
            "a benefit ARN of this catalog",
            expected_arn(CATALOG, BENEFIT_RESOURCE, &benefit),
        ),
        ("an application ARN of this catalog", application),
        (
            "an application ARN of an application that does not exist",
            expected_arn(CATALOG, APPLICATION_RESOURCE, "benappl-00000000000000"),
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:::a-bucket-of-another-service".to_string(),
        ),
    ];

    for (case, identifier) in &rows {
        refused(
            &harness,
            case,
            CATALOG,
            identifier,
            &not_found_message(ALLOCATION_KIND, identifier),
        )
        .await;
    }
}

/// Rule 5: a catalog the service does not serve is refused with the catalog-lookup message,
/// whatever the identifier — the catalog is looked up before the allocation is.
#[tokio::test]
async fn a_catalog_the_service_does_not_serve_is_refused() {
    let harness = Harness::new();
    let stored = stored_allocations(&harness, CATALOG)
        .first()
        .map(|allocation| allocation.id.clone())
        .expect("the seeded catalog holds allocations");

    // The `CatalogName` pattern `^[A-Za-z0-9_-]+$` admits all of these; none is a catalog
    // this service serves (`SEEDED_CATALOGS`), including the lower-case spelling of one.
    for catalog in ["NotACatalog", "aws"] {
        for identifier in [stored.as_str(), UNKNOWN_ALLOCATION] {
            let case = format!("{catalog}/{identifier}");
            refused(
                &harness,
                &case,
                catalog,
                identifier,
                &not_found_message(CATALOG_KIND, catalog),
            )
            .await;
        }
    }
}

/// Rule 6: a read materialises only the expiry the clock already made due — the allocation
/// past its `ExpiresAt` is never reported `ACTIVE`, a credit code past its own is never
/// reported redeemable, and a second read reports exactly what the first did.
#[tokio::test]
async fn a_read_materialises_a_due_expiry_and_changes_nothing_else() {
    let harness = Harness::new();

    // Every seeded allocation: the one whose term closed before state init reports the
    // expiry, those inside their term keep the status they were granted with, and two reads
    // agree in every case.
    let mut expired = 0usize;
    let mut live = 0usize;
    for allocation in stored_allocations(&harness, CATALOG) {
        let case = format!("seeded {}", allocation.id);
        let first = get(&harness, &case, CATALOG, &allocation.id).await;
        let second = get(&harness, &case, CATALOG, &allocation.id).await;
        assert_eq!(
            second, first,
            "case {case:?}: a second read must report exactly what the first did, \
             got {second:?} and {first:?}"
        );
        let status = member(&first, "Status").unwrap_or_default();
        let expires_at = instant(&case, &first, "ExpiresAt");
        if expires_at <= now_millis() {
            expired += 1;
            assert_ne!(
                status, ALLOCATION_STATUS_ACTIVE,
                "case {case:?}: an allocation past its ExpiresAt is never ACTIVE, got {first:?}"
            );
            if status == ALLOCATION_STATUS_INACTIVE {
                assert_eq!(
                    member(&first, "StatusReason").as_deref(),
                    Some(ALLOCATION_EXPIRED_REASON),
                    "case {case:?}: an expired allocation explains its status, got {first:?}"
                );
            }
        } else {
            live += 1;
            assert!(
                status == ALLOCATION_STATUS_ACTIVE || status == ALLOCATION_STATUS_FULFILLED,
                "case {case:?}: an allocation inside its term keeps the status it was \
                 granted with, got {status:?}"
            );
        }
    }
    assert!(
        expired > 0 && live > 0,
        "the table must cover an allocation past its term and one inside it, got \
         {expired} expired and {live} live"
    );

    // A credit code expires on the same rule, against its OWN ExpiresAt: the issued code of
    // the allocation below is placed past its term directly in the store (AWS's fulfillment
    // workflow is what issues a code, and no operation this emulator serves moves one), and
    // the read materialises the expiry the clock then owes.
    let with_codes = stored_allocations(&harness, CATALOG)
        .into_iter()
        .find(|allocation| {
            matches!(&allocation.fulfillment, Some(AllocationFulfillment::Credits { codes, .. }) if !codes.is_empty())
        })
        .expect("a seeded allocation has issued a credit code");
    let case = format!("expired code of {}", with_codes.id);
    let before = get(&harness, &case, CATALOG, &with_codes.id).await;
    for code in before["FulfillmentDetail"]["CreditDetails"]["Codes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected credit codes, got {before:?}"))
    {
        assert_eq!(
            member(code, "Status").as_deref(),
            Some(ALLOCATION_STATUS_ACTIVE),
            "case {case:?}: a code inside its term is redeemable, got {code:?}"
        );
    }

    place_expiry(&harness, CATALOG, &with_codes.id, now_millis() - 1);
    let after = get(&harness, &case, CATALOG, &with_codes.id).await;
    for code in after["FulfillmentDetail"]["CreditDetails"]["Codes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected credit codes, got {after:?}"))
    {
        assert_eq!(
            member(code, "Status").as_deref(),
            Some(ALLOCATION_STATUS_INACTIVE),
            "case {case:?}: a code past its ExpiresAt can no longer be redeemed, got {code:?}"
        );
    }
    assert_eq!(
        get(&harness, &case, CATALOG, &with_codes.id).await,
        after,
        "case {case:?}: once the expiry is materialised a further read changes nothing"
    );
}

/// Put one stored allocation's term end at `expires_at_millis`, and its credit codes' with
/// it, directly in the store.
///
/// AWS's own fulfillment workflow is what grants an allocation and issues its codes, and no
/// operation this emulator serves moves either, so a rule stated over a term that has
/// closed can only be exercised by placing the instant here. The behaviour under test is
/// still read and asserted over the wire.
fn place_expiry(harness: &Harness, catalog: &str, id: &str, expires_at_millis: i64) {
    let state = service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state");
    let mut store = state.lock();
    let allocation = store
        .catalogs
        .get_mut(catalog)
        .and_then(|catalog| catalog.allocations.get_mut(id))
        .unwrap_or_else(|| panic!("case {id:?}: names no stored allocation of {catalog:?}"));
    allocation.expires_at_millis = expires_at_millis;
    if let Some(AllocationFulfillment::Credits { codes, .. }) = allocation.fulfillment.as_mut() {
        for code in codes.iter_mut() {
            code.expires_at_millis = expires_at_millis;
        }
    }
}
