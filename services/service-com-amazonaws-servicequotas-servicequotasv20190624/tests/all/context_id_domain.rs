// SPDX-License-Identifier: CC0-1.0

//! The DOMAIN of `ContextId`: the member selects a quota's resource plane, and the only
//! values that name one are an Amazon Resource Name or the every-resource `*`.
//!
//! The rule under test, over a TABLE rather than one recorded call: a `ContextId` that
//! names no resource selects no plane, so no quota value may be put in force on it and
//! no record may report it — a quota increase request that carries one is stored and
//! then settled to the `INVALID_REQUEST` status the model declares for exactly that
//! failure, with no support case and nothing applied, while a read that carries one is
//! refused with the modelled `IllegalArgumentException`. A value that DOES name a
//! resource keeps behaving as before.
//!
//! The rows span the ways a value names a resource (an ARN of the quota's own service,
//! an ARN in another AWS partition, an ARN with neither region nor account, the
//! wildcard) and the ways one does not (a bare word, a resource id, an ARN missing a
//! segment, an empty service namespace, an empty resource part, a partition that is not
//! an AWS one, a misspelled prefix, a decorated wildcard, the empty value). The
//! harness (`Client`, `walk_pages`) is the one `quota_workflow.rs` owns.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_servicequotas_servicequotasv20190624::service::ILLEGAL_VALUE_MESSAGE;
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    CONTEXT_ID_EVERY_RESOURCE, LEVEL_ACCOUNT, LEVEL_RESOURCE, STATUS_APPROVED,
    STATUS_INVALID_REQUEST, STATUS_PENDING,
};

use crate::error_envelope::error_of;
use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};
use crate::resource_quota_context::quota_code_for;

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open), so a request
/// made before this wait has settled afterwards.
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// The quota the catalog publishes as adjustable per resource AND account-wide, so a
/// `ContextId` is a member this quota reads at all (`src/state.rs`, the `es` rows —
/// `QuotaContextInfo`'s own worked example).
const RESOURCE_QUOTA_NAME: &str = "Instances per domain";

/// One row of the table: a `ContextId` value, and whether it names a resource.
struct Row {
    case: &'static str,
    context_id: String,
    names_a_resource: bool,
}

/// Every `ContextId` the table spans, each with the case it stands for.
///
/// The values that name a resource are ARNs in the forms AWS publishes them in — a
/// regional resource of the account, another partition, and a resource whose ARN
/// carries neither a region nor an account — plus the wildcard the member's own
/// documentation names beside the ARN.
fn rows() -> Vec<Row> {
    let row = |case: &'static str, context_id: String, names_a_resource: bool| Row {
        case,
        context_id,
        names_a_resource,
    };
    vec![
        row(
            "an ARN of one resource of the quota's own service",
            format!("arn:aws:es:us-east-1:{ACCOUNT}:domain/alpha"),
            true,
        ),
        row(
            "an ARN in another AWS partition",
            format!("arn:aws-us-gov:es:us-gov-west-1:{ACCOUNT}:domain/beta"),
            true,
        ),
        row(
            "an ARN that carries neither a region nor an account",
            "arn:aws:s3:::quota-context-bucket".to_string(),
            true,
        ),
        row(
            "the every-resource wildcard",
            CONTEXT_ID_EVERY_RESOURCE.to_string(),
            true,
        ),
        row("a bare word", "not-an-arn".to_string(), false),
        row(
            "a resource id rather than its ARN",
            "vpc-0123456789abcdef0".to_string(),
            false,
        ),
        row(
            "an ARN missing its resource segment",
            format!("arn:aws:es:us-east-1:{ACCOUNT}"),
            false,
        ),
        row(
            "an ARN whose resource part is empty",
            format!("arn:aws:es:us-east-1:{ACCOUNT}:"),
            false,
        ),
        row(
            "an ARN whose service namespace is empty",
            format!("arn:aws::us-east-1:{ACCOUNT}:domain/alpha"),
            false,
        ),
        row(
            "a partition that is not an AWS one",
            format!("arn:notaws:es:us-east-1:{ACCOUNT}:domain/alpha"),
            false,
        ),
        row(
            "the ARN prefix misspelled",
            format!("rn:aws:es:us-east-1:{ACCOUNT}:domain/alpha"),
            false,
        ),
        row(
            "the wildcard with something appended",
            "*/alpha".to_string(),
            false,
        ),
        row("the empty value", String::new(), false),
    ]
}

/// The AWS default value of one quota, which is what is in force before any increase.
async fn default_value(caller: &Client, service_code: &str, quota_code: &str) -> f64 {
    caller
        .ok(
            "GetAWSDefaultServiceQuota",
            json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
        )
        .await["Quota"]["Value"]
        .as_f64()
        .expect("the default quota reports a Value")
}

/// A read that names a resource answers for that plane; a read whose `ContextId` names
/// none is refused with the modelled `IllegalArgumentException`, in this service's own
/// one sentence for an illegal value.
#[tokio::test]
async fn a_read_is_served_exactly_for_the_context_ids_that_name_a_resource() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let quota_code = quota_code_for(&caller, "es", RESOURCE_QUOTA_NAME).await;
    let in_force = default_value(&caller, "es", &quota_code).await;

    for row in rows() {
        let body = json!({
            "ServiceCode": "es",
            "QuotaCode": quota_code,
            "ContextId": row.context_id,
        });
        let (status, parsed) = caller.call("GetServiceQuota", &body).await;
        let case = row.case;
        if row.names_a_resource {
            assert_eq!(
                status, 200,
                "case {case} ({:?}): a ContextId that names a resource must be served, got \
                 {status} with {parsed}",
                row.context_id
            );
            let quota = parsed["Quota"].clone();
            assert_eq!(
                quota["QuotaContext"]["ContextId"],
                json!(row.context_id),
                "case {case}: the record must answer for the resource asked for, got {quota}"
            );
            assert_eq!(
                quota["QuotaAppliedAtLevel"],
                json!(LEVEL_RESOURCE),
                "case {case}: a read naming a resource answers at the RESOURCE level, got {quota}"
            );
            assert_eq!(
                quota["Value"].as_f64(),
                Some(in_force),
                "case {case}: with nothing applied the AWS default is in force, expected \
                 {in_force}, got {quota}"
            );
            continue;
        }
        let (code, message) = error_of(&parsed);
        assert_eq!(
            status, 400,
            "case {case} ({:?}): a ContextId that names no resource must be refused with HTTP \
             400, got {status} with {parsed}",
            row.context_id
        );
        assert!(
            code.contains("IllegalArgumentException"),
            "case {case}: expected IllegalArgumentException, got {code}"
        );
        assert_eq!(
            message.as_deref(),
            Some(ILLEGAL_VALUE_MESSAGE),
            "case {case}: expected this service's illegal-value sentence, got {message:?}"
        );
    }
}

/// An increase request is stored whatever its `ContextId` says, and the decision on it
/// is what the value's domain settles: `APPROVED` with the value in force for a
/// `ContextId` that names a resource, `INVALID_REQUEST` with nothing applied for one
/// that names none.
#[tokio::test]
async fn a_request_naming_no_resource_settles_to_invalid_request_and_applies_nothing() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let quota_code = quota_code_for(&caller, "es", RESOURCE_QUOTA_NAME).await;
    let default = default_value(&caller, "es", &quota_code).await;

    // Each row asks on its own plane, and every request is made before any of them is
    // decided, so each one's desired value is an increase on the AWS default.
    let table = rows();
    let asked: Vec<(Row, f64, String)> = {
        let mut asked = Vec::new();
        for (index, row) in table.into_iter().enumerate() {
            let desired = default + 10.0 + index as f64;
            let requested = caller
                .ok(
                    "RequestServiceQuotaIncrease",
                    json!({
                        "ServiceCode": "es",
                        "QuotaCode": quota_code,
                        "DesiredValue": desired,
                        "ContextId": row.context_id,
                    }),
                )
                .await["RequestedQuota"]
                .clone();
            let case = row.case;
            assert_eq!(
                requested["Status"],
                json!(STATUS_PENDING),
                "case {case}: a stored request starts PENDING, got {requested}"
            );
            assert_eq!(
                requested["QuotaRequestedAtLevel"],
                json!(LEVEL_RESOURCE),
                "case {case}: a request naming a ContextId is made at the RESOURCE level, got \
                 {requested}"
            );
            let id = requested["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected an Id, got {requested}"))
                .to_string();
            asked.push((row, desired, id));
        }
        asked
    };

    tokio::time::sleep(PAST_DECISION).await;

    for (row, desired, id) in &asked {
        let case = row.case;
        let stored = caller
            .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
            .await["RequestedQuota"]
            .clone();
        let expected = if row.names_a_resource {
            STATUS_APPROVED
        } else {
            STATUS_INVALID_REQUEST
        };
        assert_eq!(
            stored["Status"],
            json!(expected),
            "case {case} ({:?}): expected Status {expected}, got {stored}",
            row.context_id
        );
        assert_eq!(
            stored["QuotaContext"]["ContextId"],
            json!(row.context_id),
            "case {case}: the stored request must keep the ContextId it was made with, got \
             {stored}"
        );
        // A decision is terminal: reading it again answers the same record, and a
        // request nothing could process never gained a support case.
        let again = caller
            .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
            .await["RequestedQuota"]
            .clone();
        assert_eq!(
            again, stored,
            "case {case}: a second read must answer the same record, got {again}"
        );
        assert_eq!(
            stored["CaseId"].is_null(),
            !row.names_a_resource,
            "case {case}: a CaseId belongs to a request a support case processed, got {stored}"
        );

        if !row.names_a_resource {
            continue;
        }
        let applied = caller
            .ok(
                "GetServiceQuota",
                json!({
                    "ServiceCode": "es",
                    "QuotaCode": quota_code,
                    "ContextId": row.context_id,
                }),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            applied["Value"].as_f64(),
            Some(*desired),
            "case {case}: an approved increase must be in force for its resource, expected \
             {desired}, got {applied}"
        );
    }

    // The RESOURCE plane holds exactly the contexts that name a resource: nothing is
    // applied for a request the service could not process.
    let resource_records = walk_pages(
        &caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": "es", "QuotaAppliedAtLevel": LEVEL_RESOURCE}),
        "Quotas",
        None,
    )
    .await;
    let mut reported: Vec<&str> = resource_records
        .iter()
        .map(|quota| {
            quota["QuotaContext"]["ContextId"]
                .as_str()
                .unwrap_or_default()
        })
        .collect();
    reported.sort_unstable();
    let mut wanted: Vec<&str> = asked
        .iter()
        .filter(|(row, _, _)| row.names_a_resource)
        .map(|(row, _, _)| row.context_id.as_str())
        .collect();
    wanted.sort_unstable();
    assert_eq!(
        reported, wanted,
        "case the applied RESOURCE plane: expected one record per context that names a resource, \
         got {resource_records:?}"
    );

    // The account-wide record is where it was: no resource-level request reached it.
    let account_wide = caller
        .ok(
            "GetServiceQuota",
            json!({"ServiceCode": "es", "QuotaCode": quota_code}),
        )
        .await["Quota"]
        .clone();
    assert_eq!(
        account_wide["Value"].as_f64(),
        Some(default),
        "case the account-wide record: expected the AWS default {default}, got {account_wide}"
    );
    assert_eq!(
        account_wide["QuotaAppliedAtLevel"],
        json!(LEVEL_ACCOUNT),
        "case the account-wide record: it must report the ACCOUNT level, got {account_wide}"
    );
}
