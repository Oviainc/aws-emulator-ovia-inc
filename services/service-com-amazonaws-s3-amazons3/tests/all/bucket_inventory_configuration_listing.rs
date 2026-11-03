// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the LISTING of a bucket's S3 Inventory configurations:
//! `ListBucketInventoryConfigurations`.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals. The configurations are put, reported
//! and compared through the round-trip suite's own helpers
//! ([`super::bucket_inventory_configuration`]), so the two suites cannot drift on how an
//! `InventoryConfiguration` is spelled.
//!
//! The rules under test, with their sources:
//!
//! * `ListBucketInventoryConfigurations` "returns a list of S3 Inventory configurations for
//!   the bucket": the response reports exactly the configurations the puts recorded on THAT
//!   bucket, each whole, in a stable order; a bucket carrying none reports an empty,
//!   untruncated list.
//! * "This action supports list pagination and does not return more than 100 configurations
//!   at a time. Always check the IsTruncated element in the response. If there are no more
//!   configurations to list, IsTruncated is set to false. If there are more configurations to
//!   list, IsTruncated is set to true, and there is a value in NextContinuationToken. You use
//!   the NextContinuationToken value to continue the pagination of the list by passing the
//!   value in continuation-token in the request" (the operation's own documentation): the
//!   pages partition the bucket's configurations, each page echoes the token the request
//!   carried, and a token the service did not issue is refused with
//!   `MalformedContinuationToken` (HTTP 400).
//! * A listing is a REPORT: two listings of an unchanged bucket report the same thing, and
//!   neither consumes nor hides what a later read sees.
//! * A call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`.

use service_com_amazonaws_s3_amazons3::ops::types::{
    INVENTORY_FORMAT_VALUES, INVENTORY_FREQUENCY_VALUES, INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES,
    INVENTORY_OPTIONAL_FIELD_VALUES,
};
use service_com_amazonaws_s3_amazons3::state::{
    MALFORMED_CONTINUATION_TOKEN_CODE, NO_SUCH_BUCKET_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_analytics_configuration::FOREIGN_ACCOUNT_ID;
use super::bucket_inventory_configuration::{
    bucket, get_inventory, put_inventory, reported_configurations, Configuration, Destination,
    Encryption,
};
use super::bucket_lifecycle::{router, send, Answer};
use super::bucket_lifecycle_configuration::{configuration, put_lifecycle, rule};

/// One row of the served-listing table: the case, the continuation token the request
/// carries, and the headers it carries.
type ServedRow<'a> = (&'a str, Option<&'a str>, Vec<(&'a str, &'a str)>);

/// The most configurations one page of this listing carries: "this action supports list
/// pagination and does not return more than 100 configurations at a time"
/// (`ListBucketInventoryConfigurations`'s own documentation).
const CONFIGURATIONS_PER_PAGE: usize = 100;

/// `GET /<bucket>?inventory` — `ListBucketInventoryConfigurations`, optionally continuing at
/// `token` and with any extra bound headers.
pub(crate) async fn list_inventory(
    router: &MultiServiceRouter,
    bucket: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
) -> Answer {
    let uri = match token {
        None => format!("/{bucket}?inventory"),
        Some(token) => format!("/{bucket}?inventory&continuation-token={token}"),
    };
    send(router, "GET", &uri, headers, "").await
}

/// The listing body as text, for an assertion message.
fn body_of(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).into_owned()
}

/// Whether a listing reports itself truncated.
fn is_truncated(answer: &Answer) -> bool {
    answer
        .element("IsTruncated")
        .unwrap_or_else(|| panic!("a listing reports IsTruncated, got {:?}", body_of(answer)))
        == "true"
}

/// The ids a listing reports, in the order the body carries them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    reported_configurations(answer)
        .into_iter()
        .map(|reported| reported.id)
        .collect()
}

/// A plain configuration of `id`: the first value of each published catalog, no filter and no
/// optional fields — the shape a row varies FROM.
fn plain(id: &str) -> Configuration {
    Configuration::new(
        id,
        INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
        INVENTORY_FREQUENCY_VALUES[0],
        Destination::new(INVENTORY_FORMAT_VALUES[0]),
    )
}

/// Put every configuration of `configurations` on `name`, failing the case loudly if any put
/// does not answer the modelled 200.
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
            200,
            "case {case:?}: setup must put {:?}, got {} ({})",
            wanted.id,
            put.status,
            body_of(&put)
        );
    }
}

/// The table of configuration SETS every row of which a listing must report whole.
///
/// The rows differ along what the listing rule generalises over: HOW MANY configurations the
/// bucket carries (none, one, several), in what ORDER they were put (the last row puts them
/// in descending id order, so the reported order cannot be the insertion order), and WHICH
/// form each configuration takes — each published `IncludedObjectVersions`, `Frequency` and
/// destination `Format` (the catalogs the crate generates from the model, so a row cannot
/// drift from it), enabled and not, a filter prefix present and empty and absent, the
/// optional-field list absent, empty, one entry and every published entry, and each optional
/// destination member (a foreign owning account, a prefix, either encryption).
fn listing_rows() -> Vec<(&'static str, Vec<Configuration>)> {
    vec![
        ("a bucket carrying no configuration at all", vec![]),
        (
            "a bucket carrying exactly one",
            vec![plain("only").filtered("logs/")],
        ),
        (
            "every configuration form at once",
            vec![
                plain("a_plain"),
                plain("b.disabled").disabled(),
                plain("c-empty-filter-prefix").filtered(""),
                Configuration::new(
                    "d_other-catalog-values",
                    // The LAST entry of each catalog, so the row spans the published sets
                    // rather than their first value.
                    INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES
                        [INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES.len() - 1],
                    INVENTORY_FREQUENCY_VALUES[INVENTORY_FREQUENCY_VALUES.len() - 1],
                    Destination::new(INVENTORY_FORMAT_VALUES[INVENTORY_FORMAT_VALUES.len() - 1]),
                ),
                Configuration::new(
                    "e_middle-format",
                    INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
                    INVENTORY_FREQUENCY_VALUES[0],
                    Destination::new(INVENTORY_FORMAT_VALUES[1]),
                )
                .with_fields(&[]),
                plain("f-one-field").with_fields(&[INVENTORY_OPTIONAL_FIELD_VALUES[0]]),
                plain("g_every-field").with_fields(INVENTORY_OPTIONAL_FIELD_VALUES),
                Configuration::new(
                    "h-cross-account-destination",
                    INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
                    INVENTORY_FREQUENCY_VALUES[0],
                    Destination::new(INVENTORY_FORMAT_VALUES[0])
                        .owned_by(FOREIGN_ACCOUNT_ID)
                        .under("exports/"),
                ),
                Configuration::new(
                    "i_sse-s3-destination",
                    INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
                    INVENTORY_FREQUENCY_VALUES[0],
                    Destination::new(INVENTORY_FORMAT_VALUES[0]).encrypted(Encryption::Sses3),
                )
                .filtered("reports/"),
                Configuration::new(
                    "j-sse-kms-destination",
                    INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
                    INVENTORY_FREQUENCY_VALUES[0],
                    Destination::new(INVENTORY_FORMAT_VALUES[0]).encrypted(Encryption::SseKms(
                        "arn:aws:kms:us-east-1:123456789012:key/listing-key".to_string(),
                    )),
                ),
            ],
        ),
        (
            "put in descending id order",
            vec![
                plain("zebra").filtered("z/"),
                plain("middle").filtered("m/"),
                plain("alpha").filtered("a/"),
            ],
        ),
    ]
}

#[tokio::test]
async fn a_listing_reports_every_configuration_the_bucket_carries() {
    let router = router();

    for (index, (case, configurations)) in listing_rows().into_iter().enumerate() {
        let name = format!("inventory-listing-{index}");
        stand_up(&router, case, &name, &configurations).await;

        let listing = list_inventory(&router, &name, None, &[]).await;
        assert_eq!(
            listing.status,
            200,
            "case {case:?}: expected the listing to answer 200, got {} ({})",
            listing.status,
            body_of(&listing)
        );

        // What the puts recorded, in the order the listing reports them in: ascending id.
        let mut expected = configurations.clone();
        expected.sort_by(|left, right| left.id.cmp(&right.id));
        let reported = reported_configurations(&listing);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the listing to report {expected:?}, got {reported:?} ({})",
            body_of(&listing)
        );

        // `InventoryConfigurationList` is `xmlFlattened` with `xmlName`
        // `InventoryConfiguration`
        // (`smithy_query(query="operation",operation="ListBucketInventoryConfigurations")`),
        // so each entry is its own `<InventoryConfiguration>` element directly under the
        // result and no wrapper element carries the list.
        let body = body_of(&listing);
        assert!(
            !body.contains("<InventoryConfigurationList>"),
            "case {case:?}: the list is flattened, so no wrapper element carries it, got {body:?}"
        );
        assert_eq!(
            body.matches("<InventoryConfiguration>").count(),
            configurations.len(),
            "case {case:?}: expected {} flattened <InventoryConfiguration> element(s), got {body:?}",
            configurations.len()
        );

        // "If there are no more configurations to list, IsTruncated is set to false."
        assert!(
            !is_truncated(&listing),
            "case {case:?}: expected IsTruncated false for {} configuration(s), got {:?}",
            configurations.len(),
            body_of(&listing)
        );
        assert!(
            listing.element("NextContinuationToken").is_none(),
            "case {case:?}: an untruncated listing carries no NextContinuationToken, got {:?}",
            listing.element("NextContinuationToken")
        );
        // "If sent in the request, the marker that is used as a starting point for this
        // inventory configuration list response" — none was sent.
        assert!(
            listing.element("ContinuationToken").is_none(),
            "case {case:?}: expected no ContinuationToken on a first page, got {:?}",
            listing.element("ContinuationToken")
        );
    }
}

/// A listing reports one bucket's configurations and no other bucket's.
#[tokio::test]
async fn a_listing_is_scoped_to_the_bucket_it_names() {
    let router = router();

    let buckets: Vec<(&str, Vec<&str>)> = vec![
        ("inventory-scope-left", vec!["left-one", "left-two"]),
        ("inventory-scope-right", vec!["right-one"]),
        ("inventory-scope-empty", vec![]),
    ];
    for (name, ids) in &buckets {
        let configurations: Vec<Configuration> = ids
            .iter()
            .map(|id| plain(id).filtered(&format!("{id}/")))
            .collect();
        stand_up(&router, name, name, &configurations).await;
    }

    for (name, ids) in &buckets {
        let listing = list_inventory(&router, name, None, &[]).await;
        let reported = listed_ids(&listing);
        let mut expected: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {name:?}: expected the listing to report only this bucket's ids \
             ({expected:?}), got {reported:?}"
        );
    }
}

/// The pages partition the bucket's configurations: the first carries the published page
/// size, the token resumes exactly where it stopped, and the last page says so.
#[tokio::test]
async fn a_listing_pages_at_the_published_page_size() {
    let router = router();
    let name = "inventory-paging";

    // Three configurations past one page, so the second page is a PARTIAL one and the
    // boundary is crossed rather than landed on exactly.
    let total = CONFIGURATIONS_PER_PAGE + 3;
    let configurations: Vec<Configuration> = (0..total)
        // Fixed-width ids, so ascending id order is also ascending numeric order.
        .map(|index| plain(&format!("cfg-{index:04}")))
        .collect();
    stand_up(&router, "a bucket past one page", name, &configurations).await;

    let first = list_inventory(&router, name, None, &[]).await;
    assert_eq!(
        first.status,
        200,
        "case \"first page\": expected 200, got {} ({})",
        first.status,
        body_of(&first)
    );
    let first_ids = listed_ids(&first);
    assert_eq!(
        first_ids.len(),
        CONFIGURATIONS_PER_PAGE,
        "case \"first page\": expected {CONFIGURATIONS_PER_PAGE} configurations, got {}",
        first_ids.len()
    );
    assert!(
        is_truncated(&first),
        "case \"first page\": expected IsTruncated true with {total} configurations stored, \
         got {:?}",
        body_of(&first)
    );
    let token = first.element("NextContinuationToken").unwrap_or_else(|| {
        panic!(
            "case \"first page\": a truncated listing carries a NextContinuationToken, got {:?}",
            body_of(&first)
        )
    });

    let second = list_inventory(&router, name, Some(&token), &[]).await;
    assert_eq!(
        second.status,
        200,
        "case \"second page\": expected 200, got {} ({})",
        second.status,
        body_of(&second)
    );
    let second_ids = listed_ids(&second);
    assert_eq!(
        second_ids.len(),
        total - CONFIGURATIONS_PER_PAGE,
        "case \"second page\": expected the remaining {} configurations, got {}",
        total - CONFIGURATIONS_PER_PAGE,
        second_ids.len()
    );
    assert!(
        !is_truncated(&second),
        "case \"second page\": expected IsTruncated false on the last page, got {:?}",
        body_of(&second)
    );
    assert!(
        second.element("NextContinuationToken").is_none(),
        "case \"second page\": the last page carries no NextContinuationToken, got {:?}",
        second.element("NextContinuationToken")
    );
    // "If sent in the request, the marker that is used as a starting point for this inventory
    // configuration list response."
    assert_eq!(
        second.element("ContinuationToken").as_deref(),
        Some(token.as_str()),
        "case \"second page\": expected the request's own token echoed back, got {:?}",
        second.element("ContinuationToken")
    );

    // The pages PARTITION the bucket: every id exactly once, in ascending order.
    let mut walked = first_ids.clone();
    walked.extend(second_ids.clone());
    let mut expected: Vec<String> = configurations
        .iter()
        .map(|wanted| wanted.id.clone())
        .collect();
    expected.sort();
    assert_eq!(
        walked, expected,
        "case \"both pages\": expected the pages to partition the bucket's configurations \
         in ascending id order, got {walked:?}"
    );
}

/// One row of the listing-refusal table: the case, the bucket to name, the continuation
/// token to carry, the headers it carries, and the status and error code expected.
type RefusalRow<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Vec<(&'a str, &'a str)>,
    u16,
    &'a str,
);

#[tokio::test]
async fn a_listing_refuses_what_it_cannot_report() {
    let router = router();
    let name = "inventory-listing-refusals";
    stand_up(
        &router,
        "refusals",
        name,
        &[plain("kept").filtered("kept/")],
    )
    .await;
    // The account this emulator serves, read from the engine rather than spelled here, so
    // a request naming it is the owner's own request.
    let owner = engine::emulator_account_id();

    // A token this listing never issued, in each shape a client could send: not hex at
    // all, hex of something else, and hex of a sibling listing's own marker.
    let foreign_marker = hex::encode(b"s3ac1 kept");
    let rows: Vec<RefusalRow> = vec![
        (
            "a bucket this emulator does not hold",
            "inventory-listing-absent",
            None,
            vec![],
            404,
            NO_SUCH_BUCKET_CODE,
        ),
        (
            "an ExpectedBucketOwner that is not the account's",
            name,
            None,
            vec![("x-amz-expected-bucket-owner", FOREIGN_ACCOUNT_ID)],
            403,
            "AccessDenied",
        ),
        (
            "a continuation token that is not hex",
            name,
            Some("not-a-token"),
            vec![],
            400,
            MALFORMED_CONTINUATION_TOKEN_CODE,
        ),
        (
            "a continuation token this listing did not issue",
            name,
            Some("6b657074"),
            vec![],
            400,
            MALFORMED_CONTINUATION_TOKEN_CODE,
        ),
        (
            "a continuation token minted for another listing",
            name,
            Some(&foreign_marker),
            vec![],
            400,
            MALFORMED_CONTINUATION_TOKEN_CODE,
        ),
    ];

    for (case, named, token, headers, status, code) in rows {
        let answer = list_inventory(&router, named, token, &headers).await;
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

    // The account's OWN id, and an empty token (`Token` is `length {min 0}`, so it is a
    // legal value naming no cursor), are both served rather than refused.
    let served: Vec<ServedRow> = vec![
        (
            "the account's own id as ExpectedBucketOwner",
            None,
            vec![("x-amz-expected-bucket-owner", owner)],
        ),
        ("an empty continuation token", Some(""), vec![]),
    ];
    for (case, token, headers) in served {
        let answer = list_inventory(&router, name, token, &headers).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the listing to answer 200, got {} ({})",
            answer.status,
            body_of(&answer)
        );
        assert_eq!(
            listed_ids(&answer),
            vec!["kept".to_string()],
            "case {case:?}: expected the bucket's one configuration, got {:?}",
            listed_ids(&answer)
        );
    }
}

/// A listing REPORTS: it changes nothing a later caller sees.
#[tokio::test]
async fn a_listing_changes_nothing() {
    let router = router();
    let name = "inventory-listing-is-a-read";
    let configurations = vec![
        plain("first").filtered("first/"),
        Configuration::new(
            "second",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0]).under("second/"),
        )
        .with_fields(&[INVENTORY_OPTIONAL_FIELD_VALUES[0]]),
    ];
    stand_up(&router, "a read changes nothing", name, &configurations).await;

    let once = list_inventory(&router, name, None, &[]).await;
    let twice = list_inventory(&router, name, None, &[]).await;
    assert_eq!(
        reported_configurations(&once),
        reported_configurations(&twice),
        "case \"two listings of an unchanged bucket\": expected the same report, got {:?} \
         then {:?}",
        body_of(&once),
        body_of(&twice)
    );

    for wanted in &configurations {
        let read = get_inventory(&router, name, &wanted.id).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected the read after two listings to answer 200, got {} ({})",
            wanted.id,
            read.status,
            body_of(&read)
        );
        let reported = reported_configurations(&read);
        assert_eq!(
            reported,
            vec![wanted.clone()],
            "case {:?}: expected the configuration the put recorded, got {reported:?}",
            wanted.id
        );
    }
}

/// Another bucket configuration put on the same bucket neither hides nor replaces what this
/// listing reports: each bucket subresource is recorded on its own, so the inventory listing
/// reports every configuration its own puts recorded whatever else the bucket carries, and in
/// whatever order the puts arrived.
#[tokio::test]
async fn a_listing_reports_what_its_own_puts_recorded_beside_other_bucket_configuration() {
    let router = router();

    // One rule retiring the objects under a prefix — a lifecycle configuration is the other
    // prefix-scoped subresource a bucket carries, so it is the one most easily confused with
    // an inventory configuration's own filter.
    let lifecycle = configuration(&[rule(
        "<ID>retire-logs</ID><Filter><Prefix>logs/</Prefix></Filter>\
         <Status>Enabled</Status><Expiration><Days>30</Days></Expiration>",
    )]);
    let stored = vec![
        plain("reports").filtered("logs/"),
        plain("everything"),
    ];

    // Both orders, so neither put can be the one that happens to win.
    for (index, lifecycle_first) in [true, false].into_iter().enumerate() {
        let case = if lifecycle_first {
            "the lifecycle configuration put first"
        } else {
            "the inventory configurations put first"
        };
        let name = format!("inventory-beside-lifecycle-{index}");
        bucket(&router, &name).await;

        if lifecycle_first {
            let put = put_lifecycle(&router, &name, &[], &lifecycle).await;
            assert_eq!(
                put.status,
                200,
                "case {case:?}: setup must put the lifecycle configuration, got {} ({})",
                put.status,
                body_of(&put)
            );
        }
        for wanted in &stored {
            let put = put_inventory(&router, &name, wanted).await;
            assert_eq!(
                put.status,
                200,
                "case {case:?}: setup must put {:?}, got {} ({})",
                wanted.id,
                put.status,
                body_of(&put)
            );
        }
        if !lifecycle_first {
            let put = put_lifecycle(&router, &name, &[], &lifecycle).await;
            assert_eq!(
                put.status,
                200,
                "case {case:?}: setup must put the lifecycle configuration, got {} ({})",
                put.status,
                body_of(&put)
            );
        }

        let listing = list_inventory(&router, &name, None, &[]).await;
        let mut expected = stored.clone();
        expected.sort_by(|left, right| left.id.cmp(&right.id));
        let reported = reported_configurations(&listing);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the listing to report {expected:?}, got {reported:?} ({})",
            body_of(&listing)
        );
    }
}
