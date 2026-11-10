// SPDX-License-Identifier: CC0-1.0

//! `DescribeAccountConfiguration` as a REPORTING-ONLY read, over the restJson1
//! wire (GET /prod/accountConfiguration).
//!
//! `services/.../tests/all/account_configuration.rs` asserts the pair's
//! round-trip; this suite pins the one rule that is about the READ alone:
//!
//! * "Describe account configuration" (`smithy_query(query="operation",
//!   operation="DescribeAccountConfiguration")`, whose input shape
//!   `DescribeAccountConfigurationRequest` declares no members) only REPORTS, so
//!   reading the account's configuration — any number of times, for any region,
//!   whatever the account has configured — leaves every later caller seeing
//!   exactly what it would have seen had nobody read: the reported
//!   configuration, the stored configuration an update then reports, and the room
//!   left in the emulator's bounded per-region store are all unchanged by a read.
//! * The account always HAS a configuration, and the operation models no
//!   not-found error at all, so a region nothing has configured answers 200 with
//!   a configuration carrying no `kmsKeyId` rather than a refusal.
//!
//! Each rule is a TABLE over the class: the rows differ along what the account
//! has configured (nothing, a key ARN, an alias ARN, a key that was cleared
//! again), along how many times it is read, and along how many distinct regions
//! are read.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_medialive_medialive::state::MAX_ACCOUNT_CONFIGURATION_REGIONS;

use crate::wire::{call_in_region, medialive, DEFAULT_REGION};

/// The configuration a region with no customer-managed key reports: one carrying
/// no `kmsKeyId` at all, the member holding the key the CALLER created and a
/// blank value being the documented way to ask for the key MediaLive manages.
fn unconfigured() -> Value {
    json!({})
}

/// The URI both account-configuration operations are bound to:
/// `smithy_query(query="operation", operation="DescribeAccountConfiguration")`
/// is GET /prod/accountConfiguration and `UpdateAccountConfiguration` is PUT on
/// that same URI.
const ACCOUNT_CONFIGURATION: &str = "/prod/accountConfiguration";

/// A KMS key ARN `AccountConfiguration$KmsKeyId` accepts ("Specify the ARN of a
/// KMS key that you have created", `smithy_query(query="doc",
/// shape="AccountConfiguration", member="KmsKeyId")`), and an alias ARN for one.
const KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";
const ALIAS_ARN: &str = "arn:aws:kms:us-west-2:000000000000:alias/medialive-emulator";

/// One `DescribeAccountConfiguration`, signed for `region`, with the status
/// asserted first so a refusal is reported as itself rather than as a missing
/// member.
async fn reported(router: &MultiServiceRouter, case: &str, region: &str) -> Value {
    let (status, body) = call_in_region(router, region, "GET", ACCOUNT_CONFIGURATION, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeAccountConfiguration to answer 200, got {status}: {body}"
    );
    assert!(
        body.get("accountConfiguration").is_some(),
        "case {case:?}: expected an accountConfiguration member, got {body}"
    );
    body["accountConfiguration"].clone()
}

/// One `UpdateAccountConfiguration`, signed for `region`, answering its status
/// and body so a row can assert either outcome.
async fn update_in(
    router: &MultiServiceRouter,
    region: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(router, region, "PUT", ACCOUNT_CONFIGURATION, body).await
}

/// Put `region` into the state a row reads from, by applying the updates the row
/// names and asserting each one was taken.
async fn arrange(router: &MultiServiceRouter, case: &str, region: &str, keys: &[&str]) {
    for key in keys {
        let (status, body) = update_in(
            router,
            region,
            Some(json!({ "accountConfiguration": { "kmsKeyId": key } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the arranging update for {key:?} to answer 200, \
             got {status}: {body}"
        );
    }
}

/// The states an account's configuration in one region can be read in, each with
/// the updates that arrange it and the configuration it must report: nothing
/// configured, a key, an alias for one, and a key that was cleared again with
/// the documented blank value.
fn states() -> Vec<(&'static str, Vec<&'static str>, Value)> {
    vec![
        ("nothing configured", vec![], unconfigured()),
        ("a key arn", vec![KEY_ARN], json!({ "kmsKeyId": KEY_ARN })),
        (
            "an alias arn",
            vec![ALIAS_ARN],
            json!({ "kmsKeyId": ALIAS_ARN }),
        ),
        ("a key cleared again", vec![KEY_ARN, ""], unconfigured()),
    ]
}

/// Rule: the describe only reports, so reading the configuration repeatedly
/// reports the same thing every time and changes nothing a later caller would
/// have seen — the update that follows the reads reports the configuration the
/// reads found, not one the reads disturbed.
///
/// The rows cross what the account has configured with how many times it is
/// read.
#[tokio::test]
async fn repeated_reads_report_the_same_configuration_and_change_nothing() {
    for (state, arrangement, expected) in states() {
        for reads in [1_usize, 2, 5] {
            let case = format!("{state}, read {reads} times");
            let router = medialive();
            arrange(&router, &case, DEFAULT_REGION, &arrangement).await;

            for read in 0..reads {
                let described = reported(&router, &case, DEFAULT_REGION).await;
                assert_eq!(
                    described, expected,
                    "case {case:?}: expected read {read} to report {expected}, got {described}"
                );
            }

            // The state the reads left behind, seen through the operation that
            // WRITES it: an update carrying no parameter reports the stored
            // configuration, so it answers the reads' effect on the store.
            let (status, body) = update_in(&router, DEFAULT_REGION, Some(json!({}))).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update after the reads to answer 200, \
                 got {status}: {body}"
            );
            assert_eq!(
                body["accountConfiguration"], expected,
                "case {case:?}: expected the store after {reads} reads to still hold \
                 {expected}, got {body}"
            );
        }
    }
}

/// Rule: a read records nothing, so it never takes room in the emulator's
/// bounded per-region store — reading far MORE regions than the store can hold
/// leaves it as empty as it started, which an update for a fresh region proves by
/// still being accepted instead of throttled.
///
/// The rows differ along how many distinct never-configured regions are read:
/// fewer than the bound, exactly the bound, and more than it.
#[tokio::test]
async fn reading_a_region_takes_no_room_in_the_bounded_store() {
    let bound = MAX_ACCOUNT_CONFIGURATION_REGIONS;
    for regions in [1_usize, bound, bound + 2] {
        let case = format!("{regions} regions read");
        let router = medialive();

        for index in 0..regions {
            let region = format!("emulator-region-{index}");
            let described = reported(&router, &case, &region).await;
            assert_eq!(
                described,
                unconfigured(),
                "case {case:?}: expected {region} to report no kmsKeyId, got {described}"
            );
        }

        // Had each read stored a record, the store would now be full and this
        // update would be refused with TooManyRequestsException.
        let (status, body) = update_in(
            &router,
            "emulator-region-after-the-reads",
            Some(json!({ "accountConfiguration": { "kmsKeyId": KEY_ARN } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an update after the reads to answer 200, \
             got {status}: {body}"
        );
        assert_eq!(
            body["accountConfiguration"],
            json!({ "kmsKeyId": KEY_ARN }),
            "case {case:?}: expected the update to report its key, got {body}"
        );
    }
}

/// Rule: the request carries no members, so the describe answers the region's
/// stored configuration whatever the request body is — and because the account
/// always has a configuration and the operation models no not-found error, a
/// region nothing configured is answered with a configuration carrying no
/// `kmsKeyId` rather than refused.
///
/// The rows differ along the body the GET carries, crossed with whether the
/// region has a configuration.
#[tokio::test]
async fn the_read_needs_no_request_members() {
    let bodies: [(&str, Option<Value>); 2] =
        [("no body at all", None), ("an empty object", Some(json!({})))];

    for (body_case, body) in bodies {
        for configured in [false, true] {
            let case = format!(
                "{body_case} against {}",
                if configured {
                    "a configured region"
                } else {
                    "a region nothing configured"
                }
            );
            let router = medialive();
            let expected = if configured {
                arrange(&router, &case, DEFAULT_REGION, &[KEY_ARN]).await;
                json!({ "kmsKeyId": KEY_ARN })
            } else {
                unconfigured()
            };

            let (status, response) =
                call_in_region(&router, DEFAULT_REGION, "GET", ACCOUNT_CONFIGURATION, body.clone())
                    .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {response}"
            );
            assert_eq!(
                response["accountConfiguration"], expected,
                "case {case:?}: expected {expected}, got {response}"
            );
        }
    }
}
