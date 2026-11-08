// SPDX-License-Identifier: CC0-1.0

//! A stored quota increase request is resolved by `Id` on the PLANE it was made on —
//! the same plane the one-open-request rule and the applied value use.
//!
//! The rule under test, over a TABLE of quotas and region triples rather than one
//! recorded call: a quota the catalog publishes as counted account-wide (`GlobalQuota`
//! true) has ONE value, and so one request plane, for the whole account, so a request
//! on it is reported by `GetRequestedServiceQuotaChange`, by both history listings and
//! by `CreateSupportCase` at EVERY regional endpoint; a quota counted per region has one
//! value per region, so its requests are reached at the endpoint they were made at and
//! nowhere else (`NoSuchResourceException`, modelled 404 and served on the client-fault
//! 400 this awsJson front end answers a modelled client error on).
//!
//! The consequence that ties the write path to the read paths, checked for every row:
//! an endpoint never names a request `Id` it cannot resolve. Whichever id a refusal of
//! `RequestServiceQuotaIncrease` names at an endpoint, a read of that id at that same
//! endpoint answers with the record — and a case opened there lands on that one record,
//! whichever endpoint reads it back.
//!
//! The harness (`Client`, `client`, `emulator`, `walk_pages`) is the one
//! `quota_workflow.rs` owns, the error-body reader is `error_envelope.rs`'s, the request
//! reader is `support_cases.rs`'s, and the catalog lookup and the two request helpers
//! are `open_request_plane_scope.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    STATUS_APPROVED, STATUS_CASE_OPENED, STATUS_PENDING,
};

use crate::error_envelope::error_of;
use crate::open_request_plane_scope::{accepted, catalog_entry, past_decision, refusal};
use crate::quota_workflow::{client, emulator, walk_pages, Client};
use crate::support_cases::read_request;

/// The history listings, whose names are too long to repeat in every call.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// One row: a quota, the endpoint its increase is requested at, and two OTHER endpoints
/// that try to resolve the request it minted.
struct Row {
    /// Why this row is in the table.
    why: &'static str,
    service_code: &'static str,
    /// The quota, by the `QuotaName` the catalog publishes: its `QuotaCode`, AWS default
    /// `Value` and `GlobalQuota` are read through `ListAWSDefaultServiceQuotas`, so no
    /// row pins a code or a value of its own.
    quota_name: &'static str,
    /// Whether the catalog publishes this quota as counted account-wide. Asserted
    /// against the catalog's own `GlobalQuota`, so a row can never claim a scope the
    /// service does not report.
    global: bool,
    /// The endpoint the increase is requested at.
    made_at: &'static str,
    /// Two further endpoints, in other regions, which read the request back.
    read_at: [&'static str; 2],
}

/// The table: account-wide quotas of five services, over a wide spread of default
/// values and region triples (including the GovCloud and China partitions), plus
/// per-region quotas as the control.
///
/// The quota names are the catalog's (`ListServices` -> `ListAWSDefaultServiceQuotas`)
/// and every row names a quota the issue did not cite.
fn rows() -> Vec<Row> {
    vec![
        Row {
            why: "an account-wide quota of a global service, read in Asia and in Europe",
            service_code: "iam",
            quota_name: "Managed policies per account",
            global: true,
            made_at: "us-east-1",
            read_at: ["ap-southeast-2", "eu-west-3"],
        },
        Row {
            why: "an account-wide quota of a regional service, requested in Europe",
            service_code: "notifications-contacts",
            quota_name: "Email contacts per account",
            global: true,
            made_at: "eu-west-1",
            read_at: ["us-east-1", "sa-east-1"],
        },
        Row {
            why: "an account-wide quota with a small default",
            service_code: "globalaccelerator",
            quota_name: "Accelerators per account",
            global: true,
            made_at: "ap-northeast-1",
            read_at: ["us-west-2", "eu-central-1"],
        },
        Row {
            why: "an account-wide quota with a large default, across partitions",
            service_code: "budgets",
            quota_name: "Budgets",
            global: true,
            made_at: "us-gov-west-1",
            read_at: ["cn-north-1", "us-east-2"],
        },
        Row {
            why: "an account-wide quota of a content delivery service",
            service_code: "cloudfront",
            quota_name: "Alternate domain names (CNAMEs) per distribution",
            global: true,
            made_at: "il-central-1",
            read_at: ["mx-central-1", "ap-south-1"],
        },
        Row {
            why: "a PER-REGION quota: the control, reached only where it was made",
            service_code: "vpc",
            quota_name: "Subnets per VPC",
            global: false,
            made_at: "us-east-1",
            read_at: ["eu-west-1", "ap-northeast-1"],
        },
        Row {
            why: "a PER-REGION quota of a storage service",
            service_code: "s3",
            quota_name: "Replication Destinations",
            global: false,
            made_at: "eu-north-1",
            read_at: ["us-west-1", "ca-central-1"],
        },
    ]
}

/// The members of a stored request that NO read of it may differ on, whichever endpoint
/// it is read at: what the request was for, who made it and when.
///
/// The members left out are the ones the clock moves (`Status`, `LastUpdated`,
/// `CaseId`), which belong to the record's progress rather than to its identity.
const IDENTITY: &[&str] = &[
    "Id",
    "ServiceCode",
    "ServiceName",
    "QuotaCode",
    "QuotaName",
    "DesiredValue",
    "Created",
    "Requester",
    "QuotaArn",
    "GlobalQuota",
    "Unit",
    "QuotaRequestedAtLevel",
];

/// The status, error code and message of a call that must be refused: the harness's own
/// dispatch, read through the crate's one error-body reader.
async fn refused(caller: &Client, operation: &str, body: Value) -> (u16, String, String) {
    let (status, parsed) = caller.call(operation, &body).await;
    let (code, message) = error_of(&parsed);
    (status, code, message.unwrap_or_default())
}

/// Every `Id` a history listing reports at `caller`'s endpoint, walked to its last page.
async fn listed_ids(caller: &Client, operation: &str, body: &Value) -> Vec<String> {
    walk_pages(caller, operation, body, "RequestedQuotas", None)
        .await
        .iter()
        .filter_map(|record| record["Id"].as_str().map(str::to_string))
        .collect()
}

/// The request `Id` a message names: the one token in it shaped like an `Id` this
/// service mints (32 lowercase hex characters, `state::request_id`).
fn named_request_id(message: &str) -> Option<String> {
    message
        .split(|character: char| !character.is_ascii_alphanumeric())
        .find(|token| {
            token.len() == 32
                && token
                    .chars()
                    .all(|character| character.is_ascii_hexdigit() && !character.is_uppercase())
        })
        .map(str::to_string)
}

/// An account-wide quota's request is one request for the ACCOUNT: every endpoint reads
/// the same record by its `Id` and reports it in both history listings, while a
/// per-region quota's request is reached only where it was made.
#[tokio::test]
async fn a_request_resolves_at_every_endpoint_its_plane_reaches() {
    for row in rows() {
        // A fresh emulator per row, so the only request in play is this row's.
        let engine = emulator();
        let maker = client(&engine, row.made_at);
        let readers: Vec<Client> = row.read_at.iter().map(|at| client(&engine, at)).collect();

        let (quota_code, default_value, global) =
            catalog_entry(&maker, row.service_code, row.quota_name).await;
        assert_eq!(
            global, row.global,
            "case {}: the catalog must publish GlobalQuota {} for {}/{quota_code}",
            row.why, row.global, row.service_code
        );

        let desired = default_value * 2.0;
        let minted = accepted(&maker, row.service_code, &quota_code, desired).await;
        let id = minted["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: expected an Id, got {minted}", row.why))
            .to_string();
        let reference = read_request(&maker, &id).await;

        let by_quota = json!({"ServiceCode": row.service_code, "QuotaCode": quota_code});
        for (reader, at) in readers.iter().zip(row.read_at) {
            if row.global {
                // The record is the SAME record: every member of its identity reads
                // alike, wherever the id is resolved.
                let seen = read_request(reader, &id).await;
                for member in IDENTITY {
                    assert_eq!(
                        seen[member], reference[member],
                        "case {}: {member} of request {id} must read alike at {at}, expected {} \
                         got {}",
                        row.why, reference[member], seen[member]
                    );
                }
                for (operation, body) in [(HISTORY_BY_QUOTA, &by_quota), (HISTORY, &json!({}))] {
                    let listed = listed_ids(reader, operation, body).await;
                    assert!(
                        listed.contains(&id),
                        "case {}: {operation} at {at} must report request {id}, got {listed:?}",
                        row.why
                    );
                }
            } else {
                let (status, code, message) =
                    refused(reader, "GetRequestedServiceQuotaChange", json!({"RequestId": &id})).await;
                assert_eq!(
                    (status, code.contains("NoSuchResourceException")),
                    (400, true),
                    "case {}: a per-region quota's request must not resolve at {at}, got {status} \
                     {code}",
                    row.why
                );
                assert!(
                    !message.is_empty(),
                    "case {}: the refusal at {at} must carry a Message, got {message:?}",
                    row.why
                );
                for (operation, body) in [(HISTORY_BY_QUOTA, &by_quota), (HISTORY, &json!({}))] {
                    let listed = listed_ids(reader, operation, body).await;
                    assert!(
                        !listed.contains(&id),
                        "case {}: {operation} at {at} must not report another region's request \
                         {id}, got {listed:?}",
                        row.why
                    );
                }
            }
        }

        // The endpoint it was made at reads it whatever the quota's scope is, and the
        // decision the clock owes reaches every endpoint the plane does.
        assert_eq!(
            read_request(&maker, &id).await["Id"].as_str(),
            Some(id.as_str()),
            "case {}: the request must stay readable at {}",
            row.why,
            row.made_at
        );
        tokio::time::sleep(past_decision()).await;
        let decided = read_request(if row.global { &readers[1] } else { &maker }, &id).await;
        assert_eq!(
            (decided["Status"].as_str(), decided["DesiredValue"].as_f64()),
            (Some(STATUS_APPROVED), Some(desired)),
            "case {}: the decided request must report {STATUS_APPROVED} at {desired}, got {decided}",
            row.why
        );
    }
}

/// An endpoint never names a request `Id` it cannot resolve: whichever id the refusal of
/// a second request on one plane names, a read of that id at that same endpoint answers
/// with the record.
#[tokio::test]
async fn an_endpoint_resolves_every_request_id_it_names() {
    for row in rows() {
        let engine = emulator();
        let maker = client(&engine, row.made_at);
        let other = client(&engine, row.read_at[0]);

        let (quota_code, default_value, _) =
            catalog_entry(&maker, row.service_code, row.quota_name).await;
        let first = accepted(&maker, row.service_code, &quota_code, default_value * 2.0).await;
        let first_id = first["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: expected an Id, got {first}", row.why))
            .to_string();

        // The other endpoint asks for the same quota while the first request is still
        // awaiting a decision: on one account-wide plane that is refused, in the
        // service's fixed sentence for the fact — which names no request, so which
        // request holds the plane is read back through the endpoint instead.
        if row.global {
            let (status, code, message) =
                refusal(&other, row.service_code, &quota_code, default_value * 3.0).await;
            assert_eq!(
                (status, code.contains("ResourceAlreadyExistsException")),
                (400, true),
                "case {}: a second request on one account-wide plane must be refused at {}, got \
                 {status} {code}",
                row.why,
                row.read_at[0]
            );
            assert_eq!(
                named_request_id(&message),
                None,
                "case {}: the refusal must name no request of this service, got {message:?}",
                row.why
            );
            let seen = read_request(&other, &first_id).await;
            assert_eq!(
                seen["Id"].as_str(),
                Some(first_id.as_str()),
                "case {}: the endpoint that was refused must resolve the request {first_id} \
                 holding the plane, got {seen}",
                row.why
            );
        } else {
            // On a per-region plane nothing of another region is named: this endpoint's
            // own request is accepted, and the id it mints resolves here and not there.
            let second =
                accepted(&other, row.service_code, &quota_code, default_value * 3.0).await;
            let second_id = second["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {}: expected an Id, got {second}", row.why))
                .to_string();
            assert_ne!(
                second_id, first_id,
                "case {}: each region's plane must mint its own request, got {second}",
                row.why
            );
            assert_eq!(
                read_request(&other, &second_id).await["Id"].as_str(),
                Some(second_id.as_str()),
                "case {}: {} must resolve the request it minted",
                row.why,
                row.read_at[0]
            );
            let (status, code, _) = refused(
                &maker,
                "GetRequestedServiceQuotaChange",
                json!({"RequestId": &second_id}),
            )
            .await;
            assert_eq!(
                (status, code.contains("NoSuchResourceException")),
                (400, true),
                "case {}: {} must not resolve {}'s request, got {status} {code}",
                row.why,
                row.made_at,
                row.read_at[0]
            );
        }
    }
}

/// A support case opened at one endpoint lands on the ONE record its plane holds, and a
/// request another endpoint cannot resolve is refused there without changing anything.
#[tokio::test]
async fn a_support_case_is_opened_wherever_the_request_resolves() {
    for row in rows() {
        let engine = emulator();
        let maker = client(&engine, row.made_at);
        let other = client(&engine, row.read_at[1]);

        let (quota_code, default_value, _) =
            catalog_entry(&maker, row.service_code, row.quota_name).await;
        let minted = accepted(&maker, row.service_code, &quota_code, default_value * 2.0).await;
        let id = minted["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: expected an Id, got {minted}", row.why))
            .to_string();
        assert_eq!(
            minted["CaseId"],
            Value::Null,
            "case {}: a freshly minted request carries no CaseId, got {minted}",
            row.why
        );

        if row.global {
            other
                .ok("CreateSupportCase", json!({"RequestId": &id}))
                .await;
            // The case was written onto the one record the plane holds: both endpoints
            // report the same `CaseId` under the status a case being open earns.
            let at_maker = read_request(&maker, &id).await;
            let at_other = read_request(&other, &id).await;
            let case_id = at_maker["CaseId"].as_str().unwrap_or_else(|| {
                panic!(
                    "case {}: the request must carry the opened case's CaseId, got {at_maker}",
                    row.why
                )
            });
            assert_eq!(
                at_maker["Status"].as_str(),
                Some(STATUS_CASE_OPENED),
                "case {}: expected {STATUS_CASE_OPENED} after the case was opened, got {at_maker}",
                row.why
            );
            assert_eq!(
                at_other["CaseId"].as_str(),
                Some(case_id),
                "case {}: both endpoints must report the one case {case_id}, got {at_other}",
                row.why
            );
        } else {
            let (status, code, message) =
                refused(&other, "CreateSupportCase", json!({"RequestId": &id})).await;
            assert_eq!(
                (status, code.contains("NoSuchResourceException")),
                (400, true),
                "case {}: a per-region quota's request must not be reachable at {}, got {status} \
                 {code}",
                row.why,
                row.read_at[1]
            );
            assert!(
                !message.is_empty(),
                "case {}: the refusal must carry a Message, got {message:?}",
                row.why
            );
            // The refused call wrote nothing: the record is still the one that was made.
            let at_maker = read_request(&maker, &id).await;
            assert_eq!(
                (at_maker["Status"].as_str(), &at_maker["CaseId"]),
                (Some(STATUS_PENDING), &Value::Null),
                "case {}: the refused call must leave the request {STATUS_PENDING} with no \
                 CaseId, got {at_maker}",
                row.why
            );
        }
    }
}
