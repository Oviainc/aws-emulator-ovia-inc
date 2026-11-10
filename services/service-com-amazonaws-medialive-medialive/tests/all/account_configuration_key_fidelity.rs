// SPDX-License-Identifier: CC0-1.0

//! What `AccountConfiguration$KmsKeyId` reports is the ACCOUNT'S OWN value, over
//! the restJson1 wire (PUT and GET /prod/accountConfiguration).
//!
//! The rule under test, in one sentence: a read of an account-level setting
//! reports the value the last accepted write stored for the signed account and
//! region, byte for byte, and omits the member where no write has set one — the
//! service never substitutes a value of its own for a setting the caller owns.
//!
//! `KmsKeyId` is "Specifies the KMS key to use for all features that use key
//! encryption. Specify the ARN of a KMS key that you have created. Or leave
//! blank to use the key that MediaLive creates and manages for you"
//! (`smithy_query(query="doc", shape="AccountConfiguration",
//! member="KmsKeyId")`), and `UpdateAccountConfiguration` (PUT
//! /prod/accountConfiguration) and `DescribeAccountConfiguration` (GET on the
//! same URI) share that one structure on input and output
//! (`smithy_query(query="operation", operation="UpdateAccountConfiguration")`).
//! So the member carries a key the CALLER created: a blank member is itself the
//! account's state — it says the account pinned no key of its own — and the
//! service has no key of its own to report there.
//!
//! The rows span the class the rule generalises over:
//!
//! * the STORED STRING: key ARNs and alias ARNs, in each published partition,
//!   across regions, account ids and identifier spellings, including ones that
//!   differ from the request only in case or in length, each asserted byte for
//!   byte against what was written;
//! * the UNCONFIGURED STATES: a fresh account, one whose key was blanked again,
//!   one whose update was refused, and one where another region holds a key —
//!   each of which must report NO `kmsKeyId` member rather than any value, and
//!   in particular never a service-managed `alias/aws/...` key nor an ARN naming
//!   the emulator's own account id;
//! * the READ COUNT, since a report must not become a write: the same answer
//!   however many times it is read.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call_in_region, medialive, ACCOUNT, DEFAULT_REGION};

/// The URI the pair is bound to: PUT writes the configuration and GET reports it.
const ACCOUNT_CONFIGURATION: &str = "/prod/accountConfiguration";

/// Stored strings whose round trip must be exact. Each is a KMS key ARN the
/// member accepts ("the ARN of a KMS key that you have created"): a key's own
/// ARN or an alias ARN for one, in one of the partitions AWS publishes ARNs for
/// (`aws`, `aws-cn`, `aws-us-gov`). They differ along partition, region, account
/// id, which kind of KMS resource is named, the identifier's spelling, its CASE
/// and its length — every part of the string a projection could normalise or
/// rebuild instead of reporting.
const STORED_KEYS: [(&str, &str); 8] = [
    (
        "a key arn",
        "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
    ),
    (
        "a key arn in another region of the same account",
        "arn:aws:kms:eu-central-1:111122223333:key/8b2f0e5c-3a1d-4f7e-9c0b-2d6e8a4f1b37",
    ),
    (
        "an alias arn",
        "arn:aws:kms:us-west-2:444455556666:alias/broadcast-encryption",
    ),
    (
        "an alias arn whose name carries slashes",
        "arn:aws:kms:us-west-2:444455556666:alias/teams/broadcast/primary",
    ),
    (
        "a key arn in the china partition",
        "arn:aws-cn:kms:cn-north-1:777788889999:key/abcdef01-2345-6789-abcd-ef0123456789",
    ),
    (
        "an alias arn in the govcloud partition",
        "arn:aws-us-gov:kms:us-gov-west-1:222233334444:alias/MixedCase-Alias",
    ),
    (
        "an uppercase key identifier",
        "arn:aws:kms:us-west-2:111122223333:key/1234ABCD-12AB-34CD-56EF-1234567890AB",
    ),
    (
        "a long alias name",
        "arn:aws:kms:ap-northeast-1:555566667777:alias/a-very-long-alias-name-that-a-caller-may-well-have-created-for-its-own-bookkeeping",
    ),
];

/// A value the member does NOT accept, used to arrange the "an update was
/// refused" unconfigured state: a bare key identifier names no key, so the
/// update is refused and nothing is stored.
const REFUSED_KEY: &str = "1234abcd-12ab-34cd-56ef-1234567890ab";

/// One `UpdateAccountConfiguration` signed for `region`.
async fn update_in(router: &MultiServiceRouter, region: &str, body: Option<Value>) -> (u16, Value) {
    call_in_region(router, region, "PUT", ACCOUNT_CONFIGURATION, body).await
}

/// One `DescribeAccountConfiguration` signed for `region`.
async fn describe_in(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call_in_region(router, region, "GET", ACCOUNT_CONFIGURATION, None).await
}

/// The `accountConfiguration` of a response that must have answered 200.
fn configuration(case: &str, operation: &str, status: u16, body: &Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status}: {body}"
    );
    body.get("accountConfiguration").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: expected {operation} to report an accountConfiguration, got {body}")
    })
}

/// The `kmsKeyId` a configuration reports, as a client reads it: `None` when the
/// member is absent, and the exact string otherwise. A member present but not a
/// string is a defect of its own, so it is reported as the case it is.
fn reported_key(case: &str, configuration: &Value) -> Option<String> {
    match configuration.get("kmsKeyId") {
        None | Some(Value::Null) => None,
        Some(Value::String(key)) => Some(key.clone()),
        Some(other) => panic!("case {case:?}: expected kmsKeyId to be a string, got {other}"),
    }
}

/// Write `key` through the update and answer the `kmsKeyId` it reports back.
async fn configure(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    key: &str,
) -> Option<String> {
    let (status, body) = update_in(
        router,
        region,
        Some(json!({ "accountConfiguration": { "kmsKeyId": key } })),
    )
    .await;
    let configuration = configuration(case, "UpdateAccountConfiguration", status, &body);
    reported_key(case, &configuration)
}

/// Rule: the stored string is reported byte for byte — by the update that
/// accepted it and by every later read — so a caller can feed what it read back
/// into a later call unchanged.
///
/// The rows are [`STORED_KEYS`], crossed with reading the configuration once,
/// twice and five times, since a report must answer the same thing however often
/// it is asked.
#[tokio::test]
async fn a_read_reports_the_stored_key_byte_for_byte() {
    for (case, key) in STORED_KEYS {
        for reads in [1_usize, 2, 5] {
            let case = format!("{case}, read {reads} times");
            let router = medialive();

            let echoed = configure(&router, &case, DEFAULT_REGION, key).await;
            assert_eq!(
                echoed.as_deref(),
                Some(key),
                "case {case:?}: expected the update to report the key it stored, got {echoed:?}"
            );

            for read in 0..reads {
                let (status, body) = describe_in(&router, DEFAULT_REGION).await;
                let described = configuration(&case, "DescribeAccountConfiguration", status, &body);
                let actual = reported_key(&case, &described);
                assert_eq!(
                    actual.as_deref(),
                    Some(key),
                    "case {case:?}: expected read {read} to report {key:?}, got {actual:?}"
                );
            }
        }
    }
}

/// Rule: a later write replaces the stored string, and the read reports the LAST
/// one accepted rather than the first or a merge of the two.
///
/// The rows are consecutive pairs of [`STORED_KEYS`], so each row's second key
/// differs from its first in partition, region, account, resource kind or
/// spelling.
#[tokio::test]
async fn a_read_reports_the_last_accepted_write() {
    for window in STORED_KEYS.windows(2) {
        let [(first_case, first), (second_case, second)] = [window[0], window[1]];
        let case = format!("{first_case} then {second_case}");
        let router = medialive();

        configure(&router, &case, DEFAULT_REGION, first).await;
        configure(&router, &case, DEFAULT_REGION, second).await;

        let (status, body) = describe_in(&router, DEFAULT_REGION).await;
        let described = configuration(&case, "DescribeAccountConfiguration", status, &body);
        let actual = reported_key(&case, &described);
        assert_eq!(
            actual.as_deref(),
            Some(second),
            "case {case:?}: expected the last accepted key {second:?}, got {actual:?}"
        );
    }
}

/// The ways an account can hold no key in the region being read, each named with
/// the requests that arrange it: nothing written at all, a key blanked again
/// (the member's documented "leave blank"), a write refused for naming no key,
/// and a key held by ANOTHER region only.
fn unconfigured_states() -> Vec<(&'static str, Vec<(&'static str, Value)>)> {
    let (_, key) = STORED_KEYS[0];
    let (_, other_region_key) = STORED_KEYS[2];
    vec![
        ("nothing written at all", vec![]),
        (
            "a key blanked again",
            vec![
                (
                    DEFAULT_REGION,
                    json!({ "accountConfiguration": { "kmsKeyId": key } }),
                ),
                (
                    DEFAULT_REGION,
                    json!({ "accountConfiguration": { "kmsKeyId": "" } }),
                ),
            ],
        ),
        (
            "a refused write",
            vec![(
                DEFAULT_REGION,
                json!({ "accountConfiguration": { "kmsKeyId": REFUSED_KEY } }),
            )],
        ),
        (
            "a key configured in another region only",
            vec![(
                "eu-west-1",
                json!({ "accountConfiguration": { "kmsKeyId": other_region_key } }),
            )],
        ),
        (
            "an update that carries no parameter",
            vec![(DEFAULT_REGION, json!({}))],
        ),
    ]
}

/// Rule: where no write has set the member, the read omits it — the account's
/// state is "no key of my own pinned", and no value is substituted for it. In
/// particular the report never names a service-managed `alias/aws/...` key and
/// never an ARN carrying the emulator's own account id, neither of which the
/// account configured or could configure back.
///
/// The rows are [`unconfigured_states`], crossed with how many times the
/// configuration is read.
#[tokio::test]
async fn an_unconfigured_account_reports_no_kms_key_at_all() {
    for (state, arrangement) in unconfigured_states() {
        for reads in [1_usize, 3] {
            let case = format!("{state}, read {reads} times");
            let router = medialive();

            for (region, body) in &arrangement {
                let (status, response) = update_in(&router, region, Some(body.clone())).await;
                assert!(
                    status == 200 || status == 400,
                    "case {case:?}: expected the arranging update to be accepted or refused, \
                     got {status}: {response}"
                );
            }

            for read in 0..reads {
                let (status, body) = describe_in(&router, DEFAULT_REGION).await;
                let described = configuration(&case, "DescribeAccountConfiguration", status, &body);
                let actual = reported_key(&case, &described);
                assert_eq!(
                    actual, None,
                    "case {case:?}: expected read {read} to report no kmsKeyId, got {actual:?}"
                );
                assert_eq!(
                    described,
                    json!({}),
                    "case {case:?}: expected read {read} to report an empty configuration, \
                     got {described}"
                );
                let rendered = described.to_string();
                assert!(
                    !rendered.contains("alias/aws/") && !rendered.contains(ACCOUNT),
                    "case {case:?}: expected read {read} to report no key of the service's own, \
                     got {rendered}"
                );
            }
        }
    }
}
