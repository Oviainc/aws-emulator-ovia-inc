// SPDX-License-Identifier: CC0-1.0

//! The PROBE a client makes on a bucket's analytics plane FROM SCRATCH, end to end.
//!
//! A client that manages storage class analysis does not read one call: it probes the
//! bucket. It lists what is configured, writes a configuration, reads that one back, lists
//! again and finally removes it — and every step has to agree with the one before it. This
//! suite drives that whole walk through the router on a bucket that starts out carrying
//! nothing, so what is pinned is the AGREEMENT between the four operations rather than any
//! one of their bodies.
//!
//! The rules under test, with their sources:
//!
//! * A bucket that carries no analytics configuration lists none: `IsTruncated` is "false"
//!   and the result element carries no `AnalyticsConfiguration`
//!   (`ListBucketAnalyticsConfigurationsOutput`), and the per-id read of any id answers
//!   `NoSuchConfiguration` — "The specified configuration does not exist." — rather than a
//!   success with nothing in it.
//! * A configuration a put recorded is reported by BOTH readers, whole: the per-id read
//!   reports that one container and the listing names it beside every other id the bucket
//!   carries, in ascending id order.
//! * A delete removes THAT id alone: the listing stops naming it, the read of it answers
//!   `NoSuchConfiguration`, and every other id is reported exactly as before.
//! * A `continuation-token` the listing did not issue resumes nothing, so it is refused with
//!   `MalformedContinuationToken` and the sentence S3 publishes for it — never read as a
//!   cursor and never ignored.
//!
//! The walk is driven over a TABLE of buckets differing in how many configurations the probe
//! writes (one, two, three) and in which form each takes, so what is pinned is the walk
//! rather than one recorded sequence.

use service_com_amazonaws_s3_amazons3::ops::put_bucket_analytics_configuration::PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::state::{
    MALFORMED_CONTINUATION_TOKEN_CODE, MALFORMED_CONTINUATION_TOKEN_MESSAGE,
    NO_SUCH_CONFIGURATION_CODE, NO_SUCH_CONFIGURATION_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_analytics_configuration::{
    bucket, configuration, get_analytics, put_analytics, reported_configurations, Configuration,
    DataExport, Filter,
};
use super::bucket_lifecycle::{router, send, Answer};

/// `GET /<bucket>?analytics` — the bucket's whole analytics listing.
async fn probe(router: &MultiServiceRouter, name: &str) -> Answer {
    send(router, "GET", &format!("/{name}?analytics"), &[], "").await
}

/// `DELETE /<bucket>?analytics&id=<id>` — the removal of one configuration.
async fn remove(router: &MultiServiceRouter, name: &str, id: &str) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{name}?analytics&id={id}"),
        &[],
        "",
    )
    .await
}

/// The ids a listing reports, in the order its body carries them.
fn probed_ids(answer: &Answer) -> Vec<String> {
    reported_configurations(answer)
        .into_iter()
        .map(|reported| reported.id)
        .collect()
}

/// What a listing reports for `IsTruncated`.
fn truncated(answer: &Answer) -> Option<String> {
    answer.element("IsTruncated")
}

/// The configuration SETS the probe is driven over.
///
/// The rows differ in how many configurations the probe writes and in which form each takes
/// — no filter at all, each single predicate `AnalyticsFilter` declares, the `And`
/// conjunction, and an export with and without a prefix.
fn probe_rows() -> Vec<(&'static str, Vec<Configuration>)> {
    vec![
        (
            "one configuration, no filter and no export",
            vec![configuration("only", None, None)],
        ),
        (
            "two configurations, a prefix and a tag",
            vec![
                configuration(
                    "prefixed",
                    Some(Filter::Prefix("logs/".to_string())),
                    Some(DataExport::new("probe-destination", None, None)),
                ),
                configuration(
                    "tagged",
                    Some(Filter::Tag("team".to_string(), "gold".to_string())),
                    None,
                ),
            ],
        ),
        (
            "three configurations, one of them a conjunction",
            vec![
                configuration(
                    "conjoined",
                    Some(Filter::and(Some("reports/"), &[("team", "finance")])),
                    Some(DataExport::new(
                        "probe-destination",
                        None,
                        Some("exports/"),
                    )),
                ),
                configuration("plain", None, None),
                configuration("zoned", Some(Filter::Prefix("eu/".to_string())), None),
            ],
        ),
    ]
}

#[tokio::test]
async fn the_per_bucket_probe_agrees_with_every_write_and_delete() {
    let router = router();

    for (index, (case, written)) in probe_rows().into_iter().enumerate() {
        let name = format!("analytics-probe-{index}");
        bucket(&router, &name).await;

        // STEP 1 — the probe of a bucket from scratch: a listing that reports none, and a
        // read that says so rather than answering a success with nothing in it.
        let empty = probe(&router, &name).await;
        assert_eq!(
            empty.status,
            200,
            "case {case:?}: a bucket carrying none must still list, got {} ({})",
            empty.status,
            String::from_utf8_lossy(&empty.body)
        );
        assert_eq!(
            truncated(&empty),
            Some("false".to_string()),
            "case {case:?}: a complete listing reports IsTruncated false, got {:?}",
            truncated(&empty)
        );
        assert!(
            probed_ids(&empty).is_empty(),
            "case {case:?}: a bucket from scratch carries no configuration, got {:?}",
            probed_ids(&empty)
        );
        let absent = get_analytics(&router, &name, &written[0].id).await;
        assert_eq!(
            absent.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {case:?}: an id nothing was put under must answer \
             {NO_SUCH_CONFIGURATION_CODE}, got {} ({})",
            absent.error_code(),
            absent.status
        );
        assert_eq!(
            absent.error_message(),
            NO_SUCH_CONFIGURATION_MESSAGE,
            "case {case:?}: expected the message {NO_SUCH_CONFIGURATION_MESSAGE:?}, got {:?}",
            absent.error_message()
        );

        // STEP 2 — the writes, each recorded with no body at all.
        for wanted in &written {
            let put = put_analytics(&router, &name, wanted).await;
            assert_eq!(
                put.status,
                PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
                "case {case:?}: expected the put of {:?} to answer \
                 {PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
                wanted.id,
                put.status,
                String::from_utf8_lossy(&put.body)
            );
            assert!(
                put.body.is_empty(),
                "case {case:?}: a recorded put carries no body, got {:?}",
                String::from_utf8_lossy(&put.body)
            );
        }

        // STEP 3 — the two readers agree: the per-id read reports its own container whole,
        // and the listing names every id, in ascending id order.
        for wanted in &written {
            let read = get_analytics(&router, &name, &wanted.id).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: expected the read of {:?} to answer 200, got {} ({})",
                wanted.id,
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert_eq!(
                reported_configurations(&read),
                vec![wanted.clone()],
                "case {case:?}: expected the read of {:?} to report what was put, got {:?}",
                wanted.id,
                reported_configurations(&read)
            );
        }
        let listed = probe(&router, &name).await;
        let mut expected: Vec<String> = written.iter().map(|wanted| wanted.id.clone()).collect();
        expected.sort();
        assert_eq!(
            probed_ids(&listed),
            expected,
            "case {case:?}: expected the listing to name every id in id order, got {:?}",
            probed_ids(&listed)
        );
        assert_eq!(
            reported_configurations(&listed).len(),
            written.len(),
            "case {case:?}: expected one entry per configuration, got {:?}",
            probed_ids(&listed)
        );

        // STEP 4 — a token this listing never issued resumes nothing.
        let forged = send(
            &router,
            "GET",
            &format!("/{name}?analytics&continuation-token=not-a-token"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            forged.status,
            400,
            "case {case:?}: a forged token must be refused, got {} ({})",
            forged.status,
            String::from_utf8_lossy(&forged.body)
        );
        assert_eq!(
            forged.error_code(),
            MALFORMED_CONTINUATION_TOKEN_CODE,
            "case {case:?}: expected the code {MALFORMED_CONTINUATION_TOKEN_CODE}, got {}",
            forged.error_code()
        );
        assert_eq!(
            forged.error_message(),
            MALFORMED_CONTINUATION_TOKEN_MESSAGE,
            "case {case:?}: expected the message {MALFORMED_CONTINUATION_TOKEN_MESSAGE:?}, got \
             {:?}",
            forged.error_message()
        );

        // STEP 5 — the delete of ONE id takes that id alone away, and both readers agree
        // about it again.
        let removed = &written[0];
        let deletion = remove(&router, &name, &removed.id).await;
        assert_eq!(
            deletion.status,
            204,
            "case {case:?}: expected the delete of {:?} to answer 204, got {} ({})",
            removed.id,
            deletion.status,
            String::from_utf8_lossy(&deletion.body)
        );
        let after = get_analytics(&router, &name, &removed.id).await;
        assert_eq!(
            after.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {case:?}: the deleted id must answer {NO_SUCH_CONFIGURATION_CODE}, got {} ({})",
            after.error_code(),
            after.status
        );
        let relisted = probe(&router, &name).await;
        let survivors: Vec<String> = expected
            .iter()
            .filter(|id| **id != removed.id)
            .cloned()
            .collect();
        assert_eq!(
            probed_ids(&relisted),
            survivors,
            "case {case:?}: expected the delete to take {:?} alone away, got {:?}",
            removed.id,
            probed_ids(&relisted)
        );
        for wanted in written.iter().skip(1) {
            let kept = get_analytics(&router, &name, &wanted.id).await;
            assert_eq!(
                reported_configurations(&kept),
                vec![wanted.clone()],
                "case {case:?}: expected {:?} to survive the delete unchanged, got {:?}",
                wanted.id,
                reported_configurations(&kept)
            );
        }
    }
}
