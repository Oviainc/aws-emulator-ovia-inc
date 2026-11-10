// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT CONFIGURATION pair, end to end over the restJson1 wire:
//! `UpdateAccountConfiguration` (PUT /prod/accountConfiguration) and the
//! `DescribeAccountConfiguration` (GET on the same URI) that reads it back.
//!
//! Rules asserted here:
//!
//! * The account always HAS a configuration, but not always a key: a describe
//!   before any update answers 200 with a configuration carrying no `kmsKeyId`
//!   at all — never a 404, and never a key of the service's own invention — and
//!   a describe changes nothing a later caller would have seen.
//! * An update applies the parameters the request carries and leaves the ones it
//!   omits as they stood ("List of account configuration parameters to update",
//!   `smithy_query(query="shape", shape="UpdateAccountConfigurationRequest")`),
//!   and the configuration it answers with is the one the next describe answers
//!   with.
//! * `KmsKeyId` holds "the ARN of a KMS key that you have created. Or leave
//!   blank to use the key that MediaLive creates and manages for you"
//!   (`smithy_query(query="doc", shape="AccountConfiguration",
//!   member="KmsKeyId")`): a KMS key ARN is stored and reported back, a BLANK
//!   value clears the stored key, and a non-blank value that is not a KMS key
//!   ARN is refused with the operation's modelled `BadRequestException` (400)
//!   while the stored configuration stays as it was. What a read reports is the
//!   stored string byte for byte, with no member where nothing is stored.
//! * MediaLive is a regional service, so the configuration a regional endpoint
//!   reports is its own region's, and the per-region store is bounded: a region
//!   beyond the bound is answered with the modelled `TooManyRequestsException`
//!   rather than recorded.
//!
//! Each rule is a TABLE over the class: the ARN forms differ along partition,
//! region and which kind of KMS resource the ARN names, and the refused rows
//! differ along the way the value falls outside the rule.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call_in_region, error_code, error_message, medialive, DEFAULT_REGION};

/// The configuration a region with no customer-managed key reports: one carrying
/// no `kmsKeyId` at all. `KmsKeyId` holds the key the CALLER created ("Specify
/// the ARN of a KMS key that you have created. Or leave blank to use the key
/// that MediaLive creates and manages for you"), so a blank member is the state
/// of an account that pinned no key of its own, and the service reports no key
/// in its place.
fn unconfigured() -> Value {
    json!({})
}

/// The URI both operations are bound to: `smithy_query(query="operation",
/// operation="UpdateAccountConfiguration")` is PUT /prod/accountConfiguration
/// and `DescribeAccountConfiguration` is GET on that same URI.
const ACCOUNT_CONFIGURATION: &str = "/prod/accountConfiguration";

/// The KMS key ARN forms `KmsKeyId` accepts, as the member's documentation
/// states them ("the ARN of a KMS key that you have created"): a key's own ARN
/// and the ARN of an alias that resolves to one, in each of the partitions AWS
/// publishes ARNs for (`aws`, `aws-cn`, `aws-us-gov`).
const ACCEPTED_KEYS: [(&str, &str); 4] = [
    (
        "key arn",
        "arn:aws:kms:us-west-2:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab",
    ),
    (
        "alias arn",
        "arn:aws:kms:us-west-2:000000000000:alias/medialive-emulator",
    ),
    (
        "china partition key arn",
        "arn:aws-cn:kms:cn-north-1:000000000000:key/abcdef01-2345-6789-abcd-ef0123456789",
    ),
    (
        "govcloud alias arn",
        "arn:aws-us-gov:kms:us-gov-west-1:000000000000:alias/broadcast",
    ),
];

/// Values that are NOT the ARN of a KMS key, each falling outside the rule a
/// different way: not an ARN at all, a bare key identifier, a bare alias, an ARN
/// of another service, a KMS ARN naming something that is not a key, and a key
/// ARN with no key id after the prefix.
const REFUSED_KEYS: [(&str, &str); 6] = [
    ("not an arn at all", "my-medialive-key"),
    ("bare key id", "1234abcd-12ab-34cd-56ef-1234567890ab"),
    ("bare alias", "alias/medialive-emulator"),
    ("another service's arn", "arn:aws:s3:::medialive-emulator"),
    (
        "a kms arn that is not a key",
        "arn:aws:kms:us-west-2:000000000000:grant/1234abcd",
    ),
    (
        "a key arn with no key id",
        "arn:aws:kms:us-west-2:000000000000:key/",
    ),
];

/// One `UpdateAccountConfiguration`, signed for `region`.
async fn update_in(
    router: &MultiServiceRouter,
    region: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in_region(router, region, "PUT", ACCOUNT_CONFIGURATION, body).await
}

/// One `DescribeAccountConfiguration`, signed for `region`.
async fn describe_in(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call_in_region(router, region, "GET", ACCOUNT_CONFIGURATION, None).await
}

/// The `accountConfiguration` member of a 200 response, with the status asserted
/// first so a refusal is reported as itself rather than as a missing member.
fn configuration(case: &str, operation: &str, status: u16, body: &Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status}: {body}"
    );
    assert!(
        body.get("accountConfiguration").is_some(),
        "case {case:?}: expected {operation} to report an accountConfiguration, got {body}"
    );
    body["accountConfiguration"].clone()
}

/// An update that must succeed, answering the configuration it reports.
async fn applied(router: &MultiServiceRouter, case: &str, region: &str, body: Value) -> Value {
    let (status, response) = update_in(router, region, Some(body)).await;
    configuration(case, "UpdateAccountConfiguration", status, &response)
}

/// The configuration a describe reports, which must succeed.
async fn reported(router: &MultiServiceRouter, case: &str, region: &str) -> Value {
    let (status, response) = describe_in(router, region).await;
    configuration(case, "DescribeAccountConfiguration", status, &response)
}

/// Rule: the account's configuration exists whether or not anything has been
/// configured, so a describe on a fresh emulator answers 200 — not a 404 — with
/// a configuration that carries no `kmsKeyId`: the member names the key the
/// caller created, so nothing configured is reported as nothing rather than as a
/// key the account never set. Repeating the read answers exactly the same thing
/// (a read changes nothing).
#[tokio::test]
async fn a_describe_before_any_update_reports_no_kms_key() {
    let router = medialive();

    let first = reported(&router, "fresh account", DEFAULT_REGION).await;
    assert_eq!(
        first,
        unconfigured(),
        "case {case:?}: expected no kmsKeyId at all, got {first}",
        case = "fresh account"
    );

    let second = reported(&router, "fresh account read twice", DEFAULT_REGION).await;
    assert_eq!(
        second, first,
        "case {case:?}: expected the second read to report {first}, got {second}",
        case = "fresh account read twice"
    );
}

/// Rule: an update stores the `KmsKeyId` it was given and reports it back, and
/// the describe that follows reports the same value — the response is the stored
/// configuration, not an echo of one request.
///
/// The rows differ along the ARN's partition, region and the kind of KMS
/// resource it names (a key or an alias for one).
#[tokio::test]
async fn every_kms_key_arn_form_round_trips_through_the_store() {
    for (case, key) in ACCEPTED_KEYS {
        let router = medialive();

        let updated = applied(
            &router,
            case,
            DEFAULT_REGION,
            json!({ "accountConfiguration": { "kmsKeyId": key } }),
        )
        .await;
        assert_eq!(
            updated,
            json!({ "kmsKeyId": key }),
            "case {case:?}: expected the update to report the key it stored, got {updated}"
        );

        let described = reported(&router, case, DEFAULT_REGION).await;
        assert_eq!(
            described,
            json!({ "kmsKeyId": key }),
            "case {case:?}: expected the describe to report the stored key, got {described}"
        );
    }
}

/// Rule: a BLANK `KmsKeyId` is the documented way to ask for "the key that
/// MediaLive creates and manages for you", so it CLEARS a stored
/// customer-managed key: the account then reports no `kmsKeyId` at all, and it
/// stays cleared on the next read.
///
/// The rows are the key forms the clear is applied to.
#[tokio::test]
async fn a_blank_kms_key_clears_the_stored_key() {
    for (case, key) in ACCEPTED_KEYS {
        let router = medialive();

        applied(
            &router,
            case,
            DEFAULT_REGION,
            json!({ "accountConfiguration": { "kmsKeyId": key } }),
        )
        .await;

        let cleared = applied(
            &router,
            case,
            DEFAULT_REGION,
            json!({ "accountConfiguration": { "kmsKeyId": "" } }),
        )
        .await;
        assert_eq!(
            cleared,
            unconfigured(),
            "case {case:?}: expected a blank kmsKeyId to clear the customer key, got {cleared}"
        );

        let described = reported(&router, case, DEFAULT_REGION).await;
        assert_eq!(
            described,
            unconfigured(),
            "case {case:?}: expected the cleared configuration to be what is stored, got {described}"
        );
    }
}

/// Rule: the request is the LIST OF PARAMETERS TO UPDATE, so an update that does
/// not carry `kmsKeyId` leaves the stored key exactly as it was and reports it
/// unchanged — whether the request omits the member, omits the whole
/// `accountConfiguration`, or carries no body at all.
///
/// The rows differ along how much of the request is omitted, crossed with the
/// stored key the omission must preserve.
#[tokio::test]
async fn an_update_that_omits_a_parameter_leaves_it_as_it_stood() {
    let omissions: [(&str, Option<Value>); 3] = [
        (
            "an accountConfiguration without kmsKeyId",
            Some(json!({ "accountConfiguration": {} })),
        ),
        ("an empty object", Some(json!({}))),
        ("no body at all", None),
    ];

    for (key_case, key) in ACCEPTED_KEYS {
        for (omission_case, omission) in &omissions {
            let case = format!("{key_case} then {omission_case}");
            let router = medialive();

            applied(
                &router,
                &case,
                DEFAULT_REGION,
                json!({ "accountConfiguration": { "kmsKeyId": key } }),
            )
            .await;

            let (status, response) = update_in(&router, DEFAULT_REGION, omission.clone()).await;
            let updated = configuration(&case, "UpdateAccountConfiguration", status, &response);
            assert_eq!(
                updated,
                json!({ "kmsKeyId": key }),
                "case {case:?}: expected the omitted parameter to be left alone, got {updated}"
            );

            let described = reported(&router, &case, DEFAULT_REGION).await;
            assert_eq!(
                described,
                json!({ "kmsKeyId": key }),
                "case {case:?}: expected the stored key to survive the update, got {described}"
            );
        }
    }
}

/// Rule: a non-blank `KmsKeyId` that is not the ARN of a KMS key names no key,
/// so the update is refused with the operation's own modelled
/// `BadRequestException` (400) carrying the member and the value the request
/// supplied — and the configuration that was already stored is untouched.
///
/// The rows differ along the way the value falls outside the rule.
#[tokio::test]
async fn a_kms_key_that_is_not_a_key_arn_is_refused_and_stores_nothing() {
    // The configuration every row starts from, so the refusal can be shown to
    // leave the stored key alone rather than clearing it.
    let (_, stored) = ACCEPTED_KEYS[0];

    for (case, offered) in REFUSED_KEYS {
        let router = medialive();
        applied(
            &router,
            case,
            DEFAULT_REGION,
            json!({ "accountConfiguration": { "kmsKeyId": stored } }),
        )
        .await;

        let (status, body) = update_in(
            &router,
            DEFAULT_REGION,
            Some(json!({ "accountConfiguration": { "kmsKeyId": offered } })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal to answer 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains("KmsKeyId") && message.contains(offered),
            "case {case:?}: expected the message to name the member and the value \
             {offered:?}, got {message:?}"
        );

        let described = reported(&router, case, DEFAULT_REGION).await;
        assert_eq!(
            described,
            json!({ "kmsKeyId": stored }),
            "case {case:?}: expected the refused update to store nothing, got {described}"
        );
    }
}

/// Rule: the per-region store is bounded — the region is whatever the caller
/// signed for, so the emulator keeps configurations for at most
/// [`MAX_ACCOUNT_CONFIGURATION_REGIONS`] of them and answers a region beyond
/// that with the operation's own modelled `TooManyRequestsException` (429),
/// storing nothing. A region the store already holds a configuration for is
/// never refused, and neither is an update that sets no parameter at all — which
/// then reports no key, there being nothing configured for that region.
///
/// The rows are the three kinds of request a full store can receive: one for a
/// region it already holds, one that sets a parameter for a new region, and one
/// that sets nothing for a new region.
#[tokio::test]
async fn the_per_region_store_is_bounded() {
    use service_com_amazonaws_medialive_medialive::state::MAX_ACCOUNT_CONFIGURATION_REGIONS;

    let router = medialive();
    let (_, key) = ACCEPTED_KEYS[0];
    let (_, other_key) = ACCEPTED_KEYS[1];

    // Fill the store: one configuration per region, each carrying a key, so
    // every one of them holds a slot.
    for index in 0..MAX_ACCOUNT_CONFIGURATION_REGIONS {
        let region = format!("emulator-region-{index}");
        applied(
            &router,
            &region,
            &region,
            json!({ "accountConfiguration": { "kmsKeyId": key } }),
        )
        .await;
    }

    // A region already in the store changes as it always did: its update needs
    // no new room.
    let existing = applied(
        &router,
        "a region the full store already holds",
        "emulator-region-0",
        json!({ "accountConfiguration": { "kmsKeyId": other_key } }),
    )
    .await;
    assert_eq!(
        existing,
        json!({ "kmsKeyId": other_key }),
        "case {case:?}: expected the stored region to take the new key, got {existing}",
        case = "a region the full store already holds"
    );

    // A new region that sets a parameter is refused, and nothing is stored for
    // it.
    let case = "a new region on a full store";
    let (status, body) = update_in(
        &router,
        "emulator-region-overflow",
        Some(json!({ "accountConfiguration": { "kmsKeyId": key } })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case {case:?}: expected the refusal to answer 429, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "TooManyRequestsException",
        "case {case:?}: expected TooManyRequestsException, got {body}"
    );
    let described = reported(&router, case, "emulator-region-overflow").await;
    assert_eq!(
        described,
        unconfigured(),
        "case {case:?}: expected the refused region to hold no customer key, got {described}"
    );

    // An update that sets nothing needs no room at all, so even a new region is
    // answered rather than throttled — whether it omits the parameter or blanks
    // it, since both leave that region without a record.
    let quiet_rows: [(&str, &str, Value); 2] = [
        (
            "a new region that omits the parameter on a full store",
            "emulator-region-quiet-omitted",
            json!({}),
        ),
        (
            "a new region that blanks the parameter on a full store",
            "emulator-region-quiet-blanked",
            json!({ "accountConfiguration": { "kmsKeyId": "" } }),
        ),
    ];
    for (case, region, body) in quiet_rows {
        let (status, response) = update_in(&router, region, Some(body)).await;
        let quiet = configuration(case, "UpdateAccountConfiguration", status, &response);
        assert_eq!(
            quiet,
            unconfigured(),
            "case {case:?}: expected no kmsKeyId at all, got {quiet}"
        );
    }
}

/// Rule: MediaLive is a regional service, so the account configuration a
/// regional endpoint reports is the one configured through THAT endpoint: a key
/// set in one region is not reported by another, and a region nothing has
/// configured reports no key at all.
///
/// The rows are the regions, each carrying a different key form, so no row can
/// pass by reporting another's value.
#[tokio::test]
async fn the_configuration_belongs_to_the_region_it_was_set_in() {
    let configured: [(&str, &str); 3] = [
        ("us-west-2", ACCEPTED_KEYS[0].1),
        ("eu-west-1", ACCEPTED_KEYS[1].1),
        ("ap-northeast-1", ACCEPTED_KEYS[2].1),
    ];
    let router = medialive();

    for (region, key) in configured {
        applied(
            &router,
            region,
            region,
            json!({ "accountConfiguration": { "kmsKeyId": key } }),
        )
        .await;
    }

    for (region, key) in configured {
        let described = reported(&router, region, region).await;
        assert_eq!(
            described,
            json!({ "kmsKeyId": key }),
            "case {region:?}: expected the region's own key, got {described}"
        );
    }

    let untouched = reported(&router, "sa-east-1", "sa-east-1").await;
    assert_eq!(
        untouched,
        unconfigured(),
        "case {case:?}: expected a region nothing configured to report no kmsKeyId, \
         got {untouched}",
        case = "sa-east-1"
    );
}
