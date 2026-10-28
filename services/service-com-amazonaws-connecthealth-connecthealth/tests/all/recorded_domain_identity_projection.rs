// SPDX-License-Identifier: CC0-1.0

//! The ONE projection of a domain's recorded identity members, driven through both
//! operations that report it.
//!
//! The RULE under test: the `accountId` / `onboardingGeneration` pair a Domain response
//! carries beside its modelled members is ONE projection of the stored record — the same
//! two members, filled from the same two fields of the same `Domain` — so whichever
//! operation answers, those values are the ones the store holds for that domain, and the
//! members a response carries outside its modelled shape are exactly that pair.
//!
//! Driven through an operation from each place the member list is built: the
//! `CreateDomain` 201 (both of its answers — the create that mints the record and the
//! `@idempotent` repeat that resolves to it) and the `GetDomain` 200. Every reported pair
//! is compared with the record read straight out of
//! [`service_com_amazonaws_connecthealth_connecthealth::state`], so a response filled from
//! anywhere but the stored record fails here.
//!
//! The rows differ along what the rule generalises over: which optional create members the
//! domain carries, how many tags it holds, the Region the request was signed for, and
//! whether the domain is live or has a deletion in flight (still reported, in `DELETING`).
//!
//! The modelled member lists written out below are the two output shapes' own
//! (`smithy_query(query="shape",shape="CreateDomainOutput,GetDomainOutput",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//! — `CreateDomainOutput` declares nine members and `GetDomainOutput` those nine plus
//! `tags`), which is what makes "every key of a response that the model does not declare"
//! computable here.
//!
//! The harness (`Emulator`, `string_at`, `tag_map`) and the `webAppSetupConfiguration`
//! that puts a deletion in flight are the sibling suites': one way of driving this service
//! and one way of asking for a web application, not several.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state;

use crate::arn_resolution_during_domain_deletion::web_app_setup;
use crate::domain_subscription_lifecycle::{string_at, tag_map, Emulator};

/// `CreateDomainOutput`'s modelled members.
const CREATE_DOMAIN_OUTPUT_MEMBERS: [&str; 9] = [
    "domainId",
    "arn",
    "name",
    "kmsKeyArn",
    "encryptionContext",
    "status",
    "webAppUrl",
    "webAppConfiguration",
    "createdAt",
];

/// `GetDomainOutput`'s modelled members: `CreateDomainOutput`'s nine plus `tags`.
const GET_DOMAIN_OUTPUT_MEMBERS: [&str; 10] = [
    "domainId",
    "arn",
    "name",
    "kmsKeyArn",
    "encryptionContext",
    "status",
    "webAppUrl",
    "webAppConfiguration",
    "createdAt",
    "tags",
];

/// The members a Domain response carries BESIDE its modelled ones — the pair the one
/// shared projection writes, in the order it writes them.
const RECORDED_IDENTITY_MEMBERS: [&str; 2] = ["accountId", "onboardingGeneration"];

/// `DomainStatus`'s non-terminal deletion value, which a domain whose deletion is still in
/// flight is reported in.
const DELETING: &str = "DELETING";

/// One row: the create request a domain is made with, the Region it is signed for, and
/// whether its deletion is put in flight before the read-back.
struct Row {
    case: &'static str,
    region: &'static str,
    request: Value,
    deletion_in_flight: bool,
}

/// The recorded identity pair a response reports, having first asserted that the pair is
/// exactly what that response carries outside its modelled shape.
fn recorded_identity(
    case: &str,
    label: &str,
    body: &Value,
    modelled: &[&str],
) -> (String, String) {
    let unmodelled: BTreeSet<&str> = body
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?} ({label}): a JSON object, got {body:?}"))
        .keys()
        .map(String::as_str)
        .filter(|key| !modelled.contains(key))
        .collect();
    assert_eq!(
        unmodelled,
        RECORDED_IDENTITY_MEMBERS.into_iter().collect::<BTreeSet<_>>(),
        "case {case:?} ({label}): the members beside the modelled ones must be exactly \
         the recorded identity pair, got {body:?}"
    );
    (
        string_at(body, "accountId"),
        string_at(body, "onboardingGeneration"),
    )
}

/// The identity pair the STORE holds for this domain — what every response's pair must
/// be, read through the record rather than through another response.
fn stored_identity(emulator: &Emulator, case: &str, domain_id: &str) -> (String, String) {
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let store = store.lock();
    let domain = store
        .held_domain(emulator.region(), domain_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold domain {domain_id}"));
    (
        domain.account_id.clone(),
        domain.onboarding_generation.as_wire().to_string(),
    )
}

fn rows() -> Vec<Row> {
    let kms_key = "arn:aws:kms:us-west-2:000000000000:key/abcdef01-2345-6789-abcd-ef0123456789";
    vec![
        Row {
            case: "minimal",
            region: "us-east-1",
            request: json!({ "name": "clinic-a" }),
            deletion_in_flight: false,
        },
        Row {
            case: "name at the modelled maximum (64)",
            region: "eu-west-2",
            request: json!({ "name": "c".repeat(64) }),
            deletion_in_flight: false,
        },
        Row {
            case: "customer-managed key",
            region: "us-west-2",
            request: json!({ "name": "clinic-kms", "kmsKeyArn": kms_key }),
            deletion_in_flight: false,
        },
        Row {
            case: "fifty tags (the per-resource maximum)",
            region: "ap-southeast-2",
            request: json!({ "name": "clinic-tagged", "tags": tag_map(50, 8, 12) }),
            deletion_in_flight: false,
        },
        Row {
            case: "web app set up",
            region: "eu-central-1",
            request: json!({ "name": "clinic-web", "webAppSetupConfiguration": web_app_setup() }),
            deletion_in_flight: false,
        },
        Row {
            case: "every optional member at once",
            region: "us-west-2",
            request: json!({
                "name": "clinic-everything",
                "kmsKeyArn": kms_key,
                "tags": tag_map(3, 6, 6),
                "webAppSetupConfiguration": web_app_setup(),
            }),
            deletion_in_flight: false,
        },
        Row {
            // A domain with a web application is reported in the non-terminal DELETING
            // while its deletion is in flight, so its record is still there to project.
            case: "a deletion in flight",
            region: "us-east-1",
            request: json!({ "name": "clinic-deleting", "webAppSetupConfiguration": web_app_setup() }),
            deletion_in_flight: true,
        },
    ]
}

#[tokio::test]
async fn every_domain_response_projects_the_stored_identity_pair_through_one_projection() {
    for Row {
        case,
        region,
        request,
        deletion_in_flight,
    } in rows()
    {
        // A fresh emulator per row, so one row's domains are never what a later row reads
        // and no row approaches the account-wide domain cap.
        let emulator = Emulator::new(region);

        let created = emulator.create_domain(&request).await;
        let domain_id = string_at(&created, "domainId");
        let stored = stored_identity(&emulator, case, &domain_id);
        assert_eq!(
            recorded_identity(case, "CreateDomain", &created, &CREATE_DOMAIN_OUTPUT_MEMBERS),
            stored,
            "case {case:?}: the create must report the identity pair the store holds, \
             got {created:?}"
        );

        // The `@idempotent` repeat resolves to that same record and answers through the
        // same projection.
        let repeated = emulator.create_domain(&request).await;
        assert_eq!(
            string_at(&repeated, "domainId"),
            domain_id,
            "case {case:?}: a repeated create must resolve to the domain the first one \
             created, got {repeated:?}"
        );
        assert_eq!(
            recorded_identity(
                case,
                "a repeated CreateDomain",
                &repeated,
                &CREATE_DOMAIN_OUTPUT_MEMBERS
            ),
            stored,
            "case {case:?}: a repeated create must report the same identity pair, got \
             {repeated:?}"
        );

        let path = format!("/domain/{domain_id}");
        if deletion_in_flight {
            let (status, deleted) = emulator.send("DELETE", &path, None).await;
            assert_eq!(status, 200, "case {case:?}: DeleteDomain got {deleted:?}");
            assert_eq!(
                deleted.get("status").and_then(Value::as_str),
                Some(DELETING),
                "case {case:?}: this domain's deletion must still be in flight, got \
                 {deleted:?}"
            );
        }

        let (status, fetched) = emulator.send("GET", &path, None).await;
        assert_eq!(status, 200, "case {case:?}: GetDomain got {fetched:?}");
        assert_eq!(
            recorded_identity(case, "GetDomain", &fetched, &GET_DOMAIN_OUTPUT_MEMBERS),
            stored,
            "case {case:?}: the read must report the identity pair the store holds — the \
             one the create reported — got {fetched:?}"
        );
    }
}
