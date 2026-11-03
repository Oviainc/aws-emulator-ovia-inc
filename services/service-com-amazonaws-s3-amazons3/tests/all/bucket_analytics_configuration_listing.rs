// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the LISTING of a bucket's storage class analysis configurations,
//! and for the delete that a listing has to stop reporting:
//! `ListBucketAnalyticsConfigurations` and `DeleteBucketAnalyticsConfiguration`.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals. The configurations are put, reported
//! and compared through the round-trip suite's own helpers
//! ([`super::bucket_analytics_configuration`]), so the two suites cannot drift on how an
//! `AnalyticsConfiguration` is spelled.
//!
//! The rules under test, with their sources:
//!
//! * `ListBucketAnalyticsConfigurations` "Lists the analytics configurations for the
//!   bucket": the response reports exactly the configurations the puts recorded on THAT
//!   bucket, each whole, in a stable order; a bucket carrying none reports an empty,
//!   untruncated list.
//! * "This action supports list pagination and does not return more than 100 configurations
//!   at a time. You should always check the IsTruncated element in the response. If there
//!   are no more configurations to list, IsTruncated is set to false. If there are more
//!   configurations to list, IsTruncated is set to true, and there will be a value in
//!   NextContinuationToken. You use the NextContinuationToken value to continue the
//!   pagination of the list by passing the value in continuation-token in the request"
//!   (the operation's own documentation): the pages partition the bucket's configurations,
//!   each page echoes the token the request carried, and a token the service did not issue
//!   is refused with `InvalidArgument` (HTTP 400).
//! * A listing is a REPORT: two listings of an unchanged bucket report the same thing, and
//!   neither consumes nor hides what a later read sees.
//! * `DeleteBucketAnalyticsConfiguration` "Deletes an analytics configuration for the bucket
//!   (specified by the analytics configuration ID)": the id it names stops being listed and
//!   stops being readable (`NoSuchConfiguration`, HTTP 404), every other id the bucket
//!   carries is untouched, and a bucket carrying none under that id answers that same
//!   `NoSuchConfiguration`.
//! * A call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`.

use service_com_amazonaws_s3_amazons3::ops::put_bucket_analytics_configuration::PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::state::{
    MALFORMED_CONTINUATION_TOKEN_CODE, NO_SUCH_BUCKET_CODE, NO_SUCH_CONFIGURATION_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_analytics_configuration::{
    bucket, configuration, get_analytics, put_analytics, reported_configurations, Configuration,
    DataExport, Filter, FOREIGN_ACCOUNT_ID,
};
use super::bucket_lifecycle::{router, send, Answer};

/// One row of the served-listing table: the case, the continuation token the request
/// carries, and the headers it carries.
type ServedRow<'a> = (&'a str, Option<&'a str>, Vec<(&'a str, &'a str)>);

/// The most configurations one page of this listing carries: "this action supports list
/// pagination and does not return more than 100 configurations at a time"
/// (`ListBucketAnalyticsConfigurations`'s own documentation).
const CONFIGURATIONS_PER_PAGE: usize = 100;

/// `GET /<bucket>?analytics` — `ListBucketAnalyticsConfigurations`, optionally continuing at
/// `token` and with any extra bound headers.
async fn list_analytics(
    router: &MultiServiceRouter,
    bucket: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
) -> Answer {
    let uri = match token {
        None => format!("/{bucket}?analytics"),
        Some(token) => format!("/{bucket}?analytics&continuation-token={token}"),
    };
    send(router, "GET", &uri, headers, "").await
}

/// `DELETE /<bucket>?analytics&id=<id>` — `DeleteBucketAnalyticsConfiguration`, with any
/// extra bound headers.
async fn delete_analytics(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}?analytics&id={id}"),
        headers,
        "",
    )
    .await
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
        let put = put_analytics(router, name, wanted).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
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
/// form each configuration takes — no filter at all, each single predicate an
/// `AnalyticsFilter` declares, the `And` conjunction, and an export whose destination is
/// owned by another account, with and without a prefix.
fn listing_rows() -> Vec<(&'static str, Vec<Configuration>)> {
    vec![
        ("a bucket carrying no configuration at all", vec![]),
        (
            "a bucket carrying exactly one",
            vec![configuration(
                "only",
                Some(Filter::Prefix("logs/".to_string())),
                None,
            )],
        ),
        (
            "every configuration form at once",
            vec![
                configuration("a_no-filter", None, None),
                configuration(
                    "b.prefix",
                    Some(Filter::Prefix("documents/".to_string())),
                    Some(DataExport::new("analytics-destination", None, None)),
                ),
                configuration("c-empty-prefix", Some(Filter::Prefix(String::new())), None),
                configuration(
                    "d_tag",
                    Some(Filter::Tag("team".to_string(), "billing".to_string())),
                    None,
                ),
                configuration(
                    "e-empty-tag-value",
                    Some(Filter::Tag("team".to_string(), String::new())),
                    None,
                ),
                configuration(
                    "f_and",
                    Some(Filter::and(Some("reports/"), &[("team", "finance")])),
                    Some(DataExport::new(
                        "foreign-destination",
                        Some(FOREIGN_ACCOUNT_ID),
                        Some("exports/"),
                    )),
                ),
                configuration(
                    "g-cross-account",
                    None,
                    Some(DataExport::new(
                        "foreign-destination",
                        Some(FOREIGN_ACCOUNT_ID),
                        None,
                    )),
                ),
            ],
        ),
        (
            "put in descending id order",
            vec![
                configuration("zebra", Some(Filter::Prefix("z/".to_string())), None),
                configuration("middle", Some(Filter::Prefix("m/".to_string())), None),
                configuration("alpha", Some(Filter::Prefix("a/".to_string())), None),
            ],
        ),
    ]
}

#[tokio::test]
async fn a_listing_reports_every_configuration_the_bucket_carries() {
    let router = router();

    for (index, (case, configurations)) in listing_rows().into_iter().enumerate() {
        let name = format!("analytics-listing-{index}");
        stand_up(&router, case, &name, &configurations).await;

        let listing = list_analytics(&router, &name, None, &[]).await;
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
        // "This value is present if it was sent in the request" — none was.
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
        ("analytics-scope-left", vec!["left-one", "left-two"]),
        ("analytics-scope-right", vec!["right-one"]),
        ("analytics-scope-empty", vec![]),
    ];
    for (name, ids) in &buckets {
        let configurations: Vec<Configuration> = ids
            .iter()
            .map(|id| configuration(id, Some(Filter::Prefix(format!("{id}/"))), None))
            .collect();
        stand_up(&router, name, name, &configurations).await;
    }

    for (name, ids) in &buckets {
        let listing = list_analytics(&router, name, None, &[]).await;
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
    let name = "analytics-paging";

    // Three configurations past one page, so the second page is a PARTIAL one and the
    // boundary is crossed rather than landed on exactly.
    let total = CONFIGURATIONS_PER_PAGE + 3;
    let configurations: Vec<Configuration> = (0..total)
        // Fixed-width ids, so ascending id order is also ascending numeric order.
        .map(|index| configuration(&format!("cfg-{index:04}"), None, None))
        .collect();
    stand_up(&router, "a bucket past one page", name, &configurations).await;

    let first = list_analytics(&router, name, None, &[]).await;
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

    let second = list_analytics(&router, name, Some(&token), &[]).await;
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
    // "The marker that is used as a starting point for this analytics configuration list
    // response. This value is present if it was sent in the request."
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
    let name = "analytics-listing-refusals";
    stand_up(
        &router,
        "refusals",
        name,
        &[configuration(
            "kept",
            Some(Filter::Prefix("kept/".to_string())),
            None,
        )],
    )
    .await;
    // The account this emulator serves, read from the engine rather than spelled here, so
    // a request naming it is the owner's own request.
    let owner = engine::emulator_account_id();

    // A token this listing never issued, in each shape a client could send: not hex at
    // all, hex of something else, and hex of a sibling listing's own marker.
    let foreign_marker = hex::encode(b"s3mc1 kept");
    let rows: Vec<RefusalRow> = vec![
        (
            "a bucket this emulator does not hold",
            "analytics-listing-absent",
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
        let answer = list_analytics(&router, named, token, &headers).await;
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
        let answer = list_analytics(&router, name, token, &headers).await;
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
    let name = "analytics-listing-is-a-read";
    let configurations = vec![
        configuration(
            "first",
            Some(Filter::Prefix("first/".to_string())),
            Some(DataExport::new("analytics-destination", None, None)),
        ),
        configuration(
            "second",
            Some(Filter::Tag("tier".to_string(), "gold".to_string())),
            None,
        ),
    ];
    stand_up(&router, "a read changes nothing", name, &configurations).await;

    let once = list_analytics(&router, name, None, &[]).await;
    let twice = list_analytics(&router, name, None, &[]).await;
    assert_eq!(
        reported_configurations(&once),
        reported_configurations(&twice),
        "case \"two listings of an unchanged bucket\": expected the same report, got {:?} \
         then {:?}",
        body_of(&once),
        body_of(&twice)
    );

    for wanted in &configurations {
        let read = get_analytics(&router, name, &wanted.id).await;
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

/// A delete removes the id it names and nothing else: the listing stops reporting that one,
/// a read of it is refused, and every other id still reports its own configuration.
#[tokio::test]
async fn a_delete_is_reflected_in_the_listing_and_in_the_read() {
    let router = router();

    let stored: Vec<Configuration> = vec![
        configuration("alpha", Some(Filter::Prefix("alpha/".to_string())), None),
        configuration(
            "beta",
            Some(Filter::Tag("team".to_string(), String::new())),
            Some(DataExport::new(
                "foreign-destination",
                Some(FOREIGN_ACCOUNT_ID),
                Some("exports/"),
            )),
        ),
        configuration(
            "gamma",
            Some(Filter::and(Some("gamma/"), &[("tier", "gold")])),
            None,
        ),
    ];

    // One row per id that could be deleted — the first, a middle one and the last — so the
    // rule is asserted over WHICH configuration goes rather than over one of them.
    for (index, deleted) in stored.iter().enumerate() {
        let name = format!("analytics-delete-{index}");
        let deleted_id = deleted.id.as_str();
        stand_up(&router, deleted_id, &name, &stored).await;

        let deletion = delete_analytics(&router, &name, deleted_id, &[]).await;
        assert_eq!(
            deletion.status,
            204,
            "case {deleted_id:?}: expected the delete to answer the modelled 204, got {} ({})",
            deletion.status,
            body_of(&deletion)
        );
        assert!(
            deletion.body.is_empty(),
            "case {deleted_id:?}: a successful delete carries no body, got {:?}",
            body_of(&deletion)
        );

        let listing = list_analytics(&router, &name, None, &[]).await;
        let expected: Vec<Configuration> = stored
            .iter()
            .filter(|wanted| wanted.id != deleted.id)
            .cloned()
            .collect();
        let reported = reported_configurations(&listing);
        assert_eq!(
            reported, expected,
            "case {deleted_id:?}: expected the listing to report every other configuration \
             ({expected:?}), got {reported:?}"
        );

        let read = get_analytics(&router, &name, deleted_id).await;
        assert_eq!(
            read.status,
            404,
            "case {deleted_id:?}: expected the read of a deleted id to answer 404, got {} ({})",
            read.status,
            body_of(&read)
        );
        assert_eq!(
            read.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {deleted_id:?}: expected error code {NO_SUCH_CONFIGURATION_CODE:?}, got {:?}",
            read.error_code()
        );

        for wanted in expected {
            let kept = get_analytics(&router, &name, &wanted.id).await;
            assert_eq!(
                kept.status,
                200,
                "case {deleted_id:?}: expected {:?} to survive the delete, got {} ({})",
                wanted.id,
                kept.status,
                body_of(&kept)
            );
            let reported = reported_configurations(&kept);
            assert_eq!(
                reported,
                vec![wanted.clone()],
                "case {deleted_id:?}: expected {:?} to report its own configuration, got \
                 {reported:?}",
                wanted.id
            );
        }

        // A put under the deleted id records it again, so the delete freed that id rather
        // than poisoning it.
        let again = put_analytics(
            &router,
            &name,
            &configuration(
                deleted_id,
                Some(Filter::Prefix("again/".to_string())),
                None,
            ),
        )
        .await;
        assert_eq!(
            again.status,
            PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
            "case {deleted_id:?}: expected a put under the deleted id to answer \
             {PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            again.status,
            body_of(&again)
        );
        let relisted = listed_ids(&list_analytics(&router, &name, None, &[]).await);
        let mut all: Vec<String> = stored.iter().map(|wanted| wanted.id.clone()).collect();
        all.sort();
        assert_eq!(
            relisted, all,
            "case {deleted_id:?}: expected the re-put configuration to be listed again, got \
             {relisted:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_refuses_what_it_cannot_remove() {
    let router = router();
    let name = "analytics-delete-refusals";
    stand_up(
        &router,
        "delete refusals",
        name,
        &[configuration(
            "kept",
            Some(Filter::Prefix("kept/".to_string())),
            None,
        )],
    )
    .await;

    // One row per refusal: the case, the bucket to name, the id to name, the headers it
    // carries, and the status and error code expected.
    type DeleteRow<'a> = (&'a str, &'a str, &'a str, Vec<(&'a str, &'a str)>, u16, &'a str);
    let rows: Vec<DeleteRow> = vec![
        (
            "a bucket this emulator does not hold",
            "analytics-delete-absent",
            "kept",
            vec![],
            404,
            NO_SUCH_BUCKET_CODE,
        ),
        (
            "an id the bucket carries nothing under",
            name,
            "never-put",
            vec![],
            404,
            NO_SUCH_CONFIGURATION_CODE,
        ),
        (
            "an ExpectedBucketOwner that is not the account's",
            name,
            "kept",
            vec![("x-amz-expected-bucket-owner", FOREIGN_ACCOUNT_ID)],
            403,
            "AccessDenied",
        ),
    ];

    for (case, named, id, headers, status, code) in rows {
        let answer = delete_analytics(&router, named, id, &headers).await;
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
    let listing = list_analytics(&router, name, None, &[]).await;
    assert_eq!(
        listed_ids(&listing),
        vec!["kept".to_string()],
        "case \"after every refusal\": expected the stored configuration untouched, got {:?}",
        body_of(&listing)
    );

    // A second delete of the same id has nothing left to remove, so it reports that rather
    // than a second success.
    let first = delete_analytics(&router, name, "kept", &[]).await;
    assert_eq!(
        first.status,
        204,
        "case \"the first delete\": expected 204, got {} ({})",
        first.status,
        body_of(&first)
    );
    let second = delete_analytics(&router, name, "kept", &[]).await;
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
    let empty = list_analytics(&router, name, None, &[]).await;
    assert!(
        listed_ids(&empty).is_empty(),
        "case \"after the delete\": expected the bucket to carry none, got {:?}",
        body_of(&empty)
    );
    assert!(
        !is_truncated(&empty),
        "case \"after the delete\": an empty listing is not truncated, got {:?}",
        body_of(&empty)
    );
}
