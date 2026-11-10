// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DELETE of a bucket's S3 Inventory configurations:
//! `DeleteBucketInventoryConfiguration`.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals. The configurations are put, listed,
//! reported and compared through the round-trip and listing suites' own helpers
//! ([`super::bucket_inventory_configuration`] and
//! [`super::bucket_inventory_configuration_listing`]), so the suites cannot drift on how an
//! `InventoryConfiguration` is spelled.
//!
//! The rules under test, with their sources:
//!
//! * `DeleteBucketInventoryConfiguration` "deletes an S3 Inventory configuration (identified
//!   by the inventory ID) from the bucket" (the operation's own documentation): the id it
//!   names stops being read (`NoSuchConfiguration`, HTTP 404) and stops being listed, every
//!   other id the bucket carries keeps its own configuration whole, the call answers the
//!   modelled 204 with an empty body, and the freed id can be put again.
//! * A delete removes a configuration by ID alone, whatever that configuration carries, so
//!   the rows below vary the SHAPE of what is deleted (filter, optional fields, destination
//!   format and its optional members, either encryption, enabled or not) and the SPELLING of
//!   the id — up to the published ceiling.
//! * A bucket carrying none under the id named has nothing to remove and answers
//!   `NoSuchConfiguration` (HTTP 404), which is also what a second delete of one id answers;
//!   a call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`. Every refusal leaves the stored configurations exactly as they were.

use service_com_amazonaws_s3_amazons3::ops::put_bucket_inventory_configuration::PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::ops::types::{
    INVENTORY_FORMAT_VALUES, INVENTORY_FREQUENCY_VALUES, INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES,
    INVENTORY_OPTIONAL_FIELD_VALUES,
};
use service_com_amazonaws_s3_amazons3::state::{
    MAX_INVENTORY_ID_CHARS, NO_SUCH_BUCKET_CODE, NO_SUCH_CONFIGURATION_CODE,
    NO_SUCH_CONFIGURATION_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_analytics_configuration::FOREIGN_ACCOUNT_ID;
use super::bucket_inventory_configuration::{
    bucket, get_inventory, put_inventory, reported_configurations, Configuration, Destination,
    Encryption,
};
use super::bucket_inventory_configuration_listing::list_inventory;
use super::bucket_lifecycle::{router, send, Answer};

/// `DELETE /<bucket>?inventory&id=<id>` — `DeleteBucketInventoryConfiguration`, with any
/// extra bound headers.
async fn delete_inventory(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}?inventory&id={id}"),
        headers,
        "",
    )
    .await
}

/// A body as text, for an assertion message.
fn body_of(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).into_owned()
}

/// The ids a listing reports, in the order the body carries them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    reported_configurations(answer)
        .into_iter()
        .map(|reported| reported.id)
        .collect()
}

/// An id exactly [`MAX_INVENTORY_ID_CHARS`] characters long — the ceiling every bucket
/// configuration id shares, which a delete must still take.
fn id_at_the_ceiling() -> String {
    "z".repeat(MAX_INVENTORY_ID_CHARS)
}

/// The configurations every row of the table below stands a bucket up with.
///
/// Four ids, each carrying a DIFFERENT container, so a delete asserted over them is asserted
/// over what the configuration holds as well as over which id holds it: a plain one, one with
/// a filter prefix and every published optional field, one that is disabled and publishes
/// SSE-KMS-encrypted reports to a foreign-owned destination under a prefix, and one spelled at
/// the id ceiling publishing in the last published format with SSE-S3. The catalogs come from
/// the model's own enums (`INVENTORY_*_VALUES`, generated from
/// `smithy_query(query="shape",shape="InventoryFormat,InventoryFrequency,InventoryIncludedObjectVersions,InventoryOptionalField")`).
fn stored_configurations() -> Vec<Configuration> {
    let last_format = INVENTORY_FORMAT_VALUES[INVENTORY_FORMAT_VALUES.len() - 1];
    let last_frequency = INVENTORY_FREQUENCY_VALUES[INVENTORY_FREQUENCY_VALUES.len() - 1];
    let last_versions = INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES
        [INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES.len() - 1];

    vec![
        Configuration::new(
            "alpha",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0]),
        ),
        Configuration::new(
            "beta",
            last_versions,
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0]),
        )
        .filtered("beta/")
        .with_fields(INVENTORY_OPTIONAL_FIELD_VALUES),
        Configuration::new(
            "gamma",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            last_frequency,
            Destination::new(last_format)
                .owned_by(FOREIGN_ACCOUNT_ID)
                .under("gamma/")
                .encrypted(Encryption::SseKms(KMS_KEY_ARN.to_string())),
        )
        .disabled(),
        Configuration::new(
            &id_at_the_ceiling(),
            last_versions,
            last_frequency,
            Destination::new(last_format).encrypted(Encryption::Sses3),
        ),
    ]
}

/// A KMS key ARN, as `SSEKMS$KeyId` carries one: "the ID of the customer managed KMS key" — a
/// value of the request, so it is reported back as it was named.
const KMS_KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/inventory-key";

/// Put every configuration of `configurations` on `name`, failing the case loudly if any put
/// does not answer [`PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS`].
async fn stand_up(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    configurations: &[Configuration],
) {
    bucket(router, name).await;
    for wanted in configurations {
        let put = put_inventory(router, name, wanted).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: setup must put {:?}, got {} ({})",
            wanted.id,
            put.status,
            body_of(&put)
        );
    }
}

/// A delete removes the configuration the inventory id names and nothing else: the read of it
/// is refused, the listing stops reporting it, every other id still reports its own container
/// whole, and the freed id takes a put again.
#[tokio::test]
async fn a_delete_removes_the_inventory_id_it_names_and_nothing_else() {
    let router = router();
    let stored = stored_configurations();
    let owner = engine::emulator_account_id();

    // One row per id that could be deleted — the first, the middle ones and the last of the
    // stored set — so the rule is asserted over WHICH configuration goes, and over what it
    // carries, rather than over one of them. Half the rows name the account as the expected
    // bucket owner, which is the owner and so changes nothing.
    for (index, deleted) in stored.iter().enumerate() {
        let name = format!("inventory-delete-{index}");
        let deleted_id = deleted.id.as_str();
        let case = format!("deleting {deleted_id:?}");
        stand_up(&router, &case, &name, &stored).await;

        let headers: Vec<(&str, &str)> = if index % 2 == 0 {
            vec![]
        } else {
            vec![("x-amz-expected-bucket-owner", owner)]
        };
        let deletion = delete_inventory(&router, &name, deleted_id, &headers).await;
        assert_eq!(
            deletion.status,
            204,
            "case {case:?}: expected the delete to answer the modelled 204, got {} ({})",
            deletion.status,
            body_of(&deletion)
        );
        assert!(
            deletion.body.is_empty(),
            "case {case:?}: a successful delete carries no body, got {:?}",
            body_of(&deletion)
        );

        let read = get_inventory(&router, &name, deleted_id).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: expected the read of a deleted id to answer 404, got {} ({})",
            read.status,
            body_of(&read)
        );
        assert_eq!(
            read.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {case:?}: expected error code {NO_SUCH_CONFIGURATION_CODE:?}, got {:?}",
            read.error_code()
        );

        let expected: Vec<Configuration> = stored
            .iter()
            .filter(|wanted| wanted.id != deleted.id)
            .cloned()
            .collect();
        let listing = list_inventory(&router, &name, None, &[]).await;
        let reported = reported_configurations(&listing);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the listing to report every other configuration \
             ({expected:?}), got {reported:?}"
        );

        for wanted in &expected {
            let kept = get_inventory(&router, &name, &wanted.id).await;
            assert_eq!(
                kept.status,
                200,
                "case {case:?}: expected {:?} to survive the delete, got {} ({})",
                wanted.id,
                kept.status,
                body_of(&kept)
            );
            let reported = reported_configurations(&kept);
            assert_eq!(
                reported,
                vec![wanted.clone()],
                "case {case:?}: expected {:?} to report its own configuration, got {reported:?}",
                wanted.id
            );
        }

        // A put under the deleted id records it again, so the delete freed that id rather
        // than poisoning it — and what it records is the NEW container, not the removed one.
        let replacement = Configuration::new(
            deleted_id,
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0]),
        )
        .filtered("again/");
        let again = put_inventory(&router, &name, &replacement).await;
        assert_eq!(
            again.status,
            PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected a put under the deleted id to answer \
             {PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            again.status,
            body_of(&again)
        );
        let read_again = get_inventory(&router, &name, deleted_id).await;
        let reported = reported_configurations(&read_again);
        assert_eq!(
            reported,
            vec![replacement.clone()],
            "case {case:?}: expected the re-put configuration to be reported, got {reported:?}"
        );
        let relisted = listed_ids(&list_inventory(&router, &name, None, &[]).await);
        let mut all: Vec<String> = stored.iter().map(|wanted| wanted.id.clone()).collect();
        all.sort();
        assert_eq!(
            relisted, all,
            "case {case:?}: expected the re-put configuration to be listed again, got \
             {relisted:?}"
        );
    }
}

/// Every delete the service cannot carry out is refused with the code AWS documents for it,
/// and leaves the configurations the bucket already carried in place.
#[tokio::test]
async fn a_delete_refuses_what_it_cannot_remove() {
    let router = router();
    let name = "inventory-delete-refusals";
    let stored = stored_configurations();
    stand_up(&router, "delete refusals", name, &stored).await;

    // One row per refusal: the case, the bucket to name, the id to name, the headers it
    // carries, and the status and error code expected.
    type DeleteRow<'a> = (&'a str, &'a str, String, Vec<(&'a str, &'a str)>, u16, &'a str);
    let rows: Vec<DeleteRow> = vec![
        (
            "a bucket this emulator does not hold",
            "inventory-delete-absent",
            "alpha".to_string(),
            vec![],
            404,
            NO_SUCH_BUCKET_CODE,
        ),
        (
            "an id the bucket carries nothing under",
            name,
            "never-put".to_string(),
            vec![],
            404,
            NO_SUCH_CONFIGURATION_CODE,
        ),
        (
            "an id at the ceiling the bucket carries nothing under",
            name,
            "y".repeat(MAX_INVENTORY_ID_CHARS),
            vec![],
            404,
            NO_SUCH_CONFIGURATION_CODE,
        ),
        (
            "an ExpectedBucketOwner that is not the account's",
            name,
            "alpha".to_string(),
            vec![("x-amz-expected-bucket-owner", FOREIGN_ACCOUNT_ID)],
            403,
            "AccessDenied",
        ),
    ];

    for (case, named, id, headers, status, code) in rows {
        let answer = delete_inventory(&router, named, &id, &headers).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} ({})",
            answer.status,
            body_of(&answer)
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected error code {code:?}, got {:?}",
            answer.error_code()
        );
    }

    // Every refusal above left the bucket exactly as it was.
    let listing = list_inventory(&router, name, None, &[]).await;
    let reported = reported_configurations(&listing);
    let mut expected = stored.clone();
    expected.sort_by(|left, right| left.id.cmp(&right.id));
    assert_eq!(
        reported, expected,
        "case \"after every refusal\": expected the stored configurations untouched, got \
         {reported:?}"
    );

    // A second delete of one id has nothing left to remove, so it reports that rather than a
    // second success — and says so in the words AWS publishes for the code.
    let first = delete_inventory(&router, name, "alpha", &[]).await;
    assert_eq!(
        first.status,
        204,
        "case \"the first delete\": expected 204, got {} ({})",
        first.status,
        body_of(&first)
    );
    let second = delete_inventory(&router, name, "alpha", &[]).await;
    assert_eq!(
        second.status,
        404,
        "case \"a second delete of the same id\": expected 404, got {} ({})",
        second.status,
        body_of(&second)
    );
    assert_eq!(
        second.error_code(),
        NO_SUCH_CONFIGURATION_CODE,
        "case \"a second delete of the same id\": expected error code \
         {NO_SUCH_CONFIGURATION_CODE:?}, got {:?}",
        second.error_code()
    );
    assert_eq!(
        second.error_message(),
        NO_SUCH_CONFIGURATION_MESSAGE,
        "case \"a second delete of the same id\": expected message \
         {NO_SUCH_CONFIGURATION_MESSAGE:?}, got {:?}",
        second.error_message()
    );

    // Deleting every remaining id leaves the bucket carrying none again.
    for wanted in stored.iter().skip(1) {
        let answer = delete_inventory(&router, name, &wanted.id, &[]).await;
        assert_eq!(
            answer.status,
            204,
            "case \"draining {:?}\": expected 204, got {} ({})",
            wanted.id,
            answer.status,
            body_of(&answer)
        );
    }
    let empty = list_inventory(&router, name, None, &[]).await;
    assert!(
        listed_ids(&empty).is_empty(),
        "case \"after draining every id\": expected the bucket to carry none, got {:?}",
        body_of(&empty)
    );
}
