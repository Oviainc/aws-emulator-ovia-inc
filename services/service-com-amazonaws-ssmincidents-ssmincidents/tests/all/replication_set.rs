// SPDX-License-Identifier: CC0-1.0

//! The two replication-set READ paths answer from ONE store.
//!
//! The rule under test: `GetReplicationSet` and `ListReplicationSets` report the same
//! stored replication sets, because both resolve `crate::state`'s `replication_sets` map
//! — every ARN the list reports resolves through the get, every ARN the store does not
//! hold is a 404 from the get and absent from the list, and neither read changes what the
//! other one answers afterwards. `tests/all/replication_set_lifecycle.rs` pins each
//! operation on its own; what is pinned HERE is that the two never disagree, which is
//! what a client that lists then describes depends on — and what would break the moment
//! one of them grew a store of its own.
//!
//! The store is seeded directly through the shared engine because no operation this
//! emulator implements creates a replication set: `CreateReplicationSet` refuses every
//! call as deprecated (Amazon Web Services stopped accepting new Incident Manager
//! customers on November 7, 2025), which the first suite below also asserts leaves the
//! store exactly as it was.
//!
//! The table spans what the rule generalises over: how many sets the store holds, their
//! Regions, their `ReplicationSetStatus` and their deletion protection.

use aws_smithy_types::DateTime;
use serde_json::json;
use service_com_amazonaws_ssmincidents_ssmincidents::ops::types::{
    RegionInfo, REGION_STATUS_VALUES, REPLICATION_SET_STATUS_VALUES,
};
use service_com_amazonaws_ssmincidents_ssmincidents::state::{
    replication_set_arn, service_state, ReplicationSetEntry,
};

use super::incident_lifecycle::{query_encode, Emulator, Reply};

/// The canonical placeholder account id every ARN the emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The principal ARN the harness's access key id maps to; the member it fills
/// (`ReplicationSet$createdBy`) targets `Arn`.
fn principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/AKIAIOSFODNN7EXAMPLE")
}

/// One replication set to persist: its ARN id, the Regions it spans, the status it
/// carries and whether it is protected from deletion.
struct SeedCase {
    id: &'static str,
    created_secs: i64,
    regions: &'static [&'static str],
    /// Source: `REPLICATION_SET_STATUS_VALUES` in `ops/types/shared_types.rs`, i.e. the
    /// model's `ReplicationSetStatus` enum.
    status: &'static str,
    deletion_protected: bool,
}

/// Rows spanning the stored shapes: one Region and three (the model's `regions` is
/// `length 1..3`), a Region with and without a customer key, deletion protection on and
/// off, and three different `ReplicationSetStatus` values.
fn seed_cases() -> Vec<SeedCase> {
    vec![
        SeedCase {
            id: "11111111-2222-3333-4444-555555555555",
            created_secs: 1_700_000_000,
            regions: &["us-east-1"],
            status: REPLICATION_SET_STATUS_VALUES[0],
            deletion_protected: true,
        },
        SeedCase {
            id: "66666666-7777-8888-9999-aaaaaaaaaaaa",
            created_secs: 1_700_000_100,
            regions: &["us-east-1", "eu-west-1", "ap-southeast-2"],
            status: REPLICATION_SET_STATUS_VALUES[1],
            deletion_protected: false,
        },
        SeedCase {
            id: "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
            created_secs: 1_700_000_200,
            regions: &["eu-central-1", "us-west-2"],
            status: REPLICATION_SET_STATUS_VALUES[2],
            deletion_protected: false,
        },
    ]
}

/// Persist `cases` in the very map both read paths resolve.
fn seed(emulator: &Emulator, cases: &[SeedCase]) {
    let state = service_state(&*emulator.engine).expect("the ssm-incidents state store");
    let mut state = state.lock();
    for case in cases {
        let created = DateTime::from_secs(case.created_secs);
        let region_map = case
            .regions
            .iter()
            .enumerate()
            .map(|(index, region)| {
                (
                    (*region).to_string(),
                    RegionInfo {
                        // Only the first Region carries a customer key, so both the
                        // present and the absent `sseKmsKeyId` shape is stored.
                        sse_kms_key_id: (index == 0)
                            .then(|| format!("arn:aws:kms:{region}:{ACCOUNT}:key/{}", case.id)),
                        // Source: `REGION_STATUS_VALUES` in `ops/types/shared_types.rs`.
                        status: REGION_STATUS_VALUES[0].to_string(),
                        status_message: None,
                        status_update_date_time: created,
                    },
                )
            })
            .collect();
        let arn = replication_set_arn(case.id);
        state.replication_sets.insert(
            arn.clone(),
            ReplicationSetEntry {
                arn,
                region_map,
                status: case.status.to_string(),
                deletion_protected: case.deletion_protected,
                created_time: created,
                created_by: principal(),
                last_modified_time: created,
                last_modified_by: principal(),
            },
        );
    }
}

/// `GET /getReplicationSet?arn=<arn>`.
async fn get_replication_set(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .get(&format!("/getReplicationSet?arn={}", query_encode(arn)))
        .await
}

/// The ARNs `POST /listReplicationSets` reports, paging to the end.
async fn list_all(emulator: &Emulator) -> Vec<String> {
    let mut arns: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "maxResults": 1 });
        if let Some(ref value) = token {
            request["nextToken"] = json!(value);
        }
        let reply = emulator.post("/listReplicationSets", request).await;
        assert_eq!(
            reply.status, 200,
            "listing must succeed, got {} body {:?}",
            reply.status, reply.body
        );
        arns.extend(
            reply.body["replicationSetArns"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("a listed ARN must be a string: {value:?}"))
                        .to_string()
                }),
        );
        token = reply.body["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return arns;
        }
    }
}

/// Every ARN the list reports resolves through the get, member for member, for every
/// prefix of the seed table — and an ARN the store does not hold is 404 from the get and
/// absent from the list.
#[tokio::test]
async fn the_list_and_the_get_report_the_same_stored_sets() {
    let all = seed_cases();
    for count in 0..=all.len() {
        let emulator = Emulator::new();
        let cases = &all[..count];
        seed(&emulator, cases);

        let listed = list_all(&emulator).await;
        assert_eq!(
            listed.len(),
            count,
            "case {count} set(s): the list must report every stored set, got {listed:?}"
        );

        for case in cases {
            let arn = replication_set_arn(case.id);
            assert!(
                listed.contains(&arn),
                "case {count} set(s): {arn} is stored but not listed, got {listed:?}"
            );
            let reply = get_replication_set(&emulator, &arn).await;
            assert_eq!(
                reply.status, 200,
                "case {count} set(s): a listed ARN must resolve, got {} body {:?}",
                reply.status, reply.body
            );
            let reported = &reply.body["replicationSet"];
            assert_eq!(
                reported["arn"].as_str(),
                Some(arn.as_str()),
                "case {count} set(s): the get must report the ARN asked for, got {:?}",
                reply.body
            );
            assert_eq!(
                reported["status"].as_str(),
                Some(case.status),
                "case {count} set(s): the get must report the STORED status, got {:?}",
                reply.body
            );
            assert_eq!(
                reported["deletionProtected"].as_bool(),
                Some(case.deletion_protected),
                "case {count} set(s): the get must report the stored deletionProtected, \
                 got {:?}",
                reply.body
            );
            let regions = reported["regionMap"]
                .as_object()
                .unwrap_or_else(|| panic!("case {count} set(s): expected a regionMap"));
            assert_eq!(
                regions.len(),
                case.regions.len(),
                "case {count} set(s): the get must report every stored Region, got {:?}",
                reply.body
            );
            for region in case.regions {
                assert!(
                    regions.contains_key(*region),
                    "case {count} set(s): {region} is stored but not reported, got {:?}",
                    reply.body
                );
            }
        }

        // An ARN of the same shape that the store holds nothing under: the get answers
        // the modelled 404 and the list never mentions it.
        let absent = replication_set_arn("00000000-0000-0000-0000-000000000000");
        let reply = get_replication_set(&emulator, &absent).await;
        assert_eq!(
            reply.status, 404,
            "case {count} set(s): an ARN the store does not hold must be 404, got {} \
             body {:?}",
            reply.status, reply.body
        );
        assert!(
            !listed.contains(&absent),
            "case {count} set(s): an ARN the store does not hold must not be listed, \
             got {listed:?}"
        );
    }
}

/// Neither read changes what the other answers afterwards, and the deprecated create
/// adds nothing for them to disagree about.
#[tokio::test]
async fn reading_either_way_leaves_the_other_answer_unchanged() {
    let cases = seed_cases();
    let emulator = Emulator::new();
    seed(&emulator, &cases);

    let listed_first = list_all(&emulator).await;
    for case in &cases {
        let arn = replication_set_arn(case.id);
        let reply = get_replication_set(&emulator, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case {arn}: the stored set must resolve, got {} body {:?}",
            reply.status, reply.body
        );
    }

    // The create is refused as deprecated, so it cannot add a set the list would show.
    let refused = emulator
        .post(
            "/createReplicationSet",
            json!({ "regions": { "us-east-1": {} } }),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"deprecated create\": expected 400, got {} body {:?}",
        refused.status, refused.body
    );

    let listed_again = list_all(&emulator).await;
    assert_eq!(
        listed_again, listed_first,
        "case \"read, get, refused create\": the listed ARNs must not change"
    );
}
