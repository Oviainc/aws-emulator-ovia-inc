// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's Spot Instance data feed —
//! `CreateSpotDatafeedSubscription` and `DescribeSpotDatafeedSubscription` — driven over the
//! wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test are:
//!
//! 1. The data feed is a SINGLETON RESOURCE, not a setting with a default: "you can create
//!    one data feed per Amazon Web Services account"
//!    (`CreateSpotDatafeedSubscription`). A create stores the account's one feed — bucket,
//!    prefix, the account as `ownerId` and the `DatafeedSubscriptionState` member `Active` —
//!    the describe REPORTS whatever the last create left, and an account that has created
//!    none is refused by the describe with EC2's `InvalidSpotDatafeed.NotFound` rather than
//!    answered with an empty result.
//! 2. `Bucket` has to be a name Amazon S3 could hold a bucket under — "for more information
//!    about bucket names, see Bucket naming rules in the Amazon S3 User Guide"
//!    (`CreateSpotDatafeedSubscriptionRequest$Bucket`) — and a `Prefix` is the leading part
//!    of an S3 object key, so neither may be a value S3 would not take; a request that
//!    carries one is refused with EC2's `InvalidParameterValue` and stores nothing.
//! 3. The describe only REPORTS, so repeated describes with no create between them report
//!    the same feed, and a `DryRun` of either operation performs nothing.
//! 4. The ONE record the store holds is charged against the emulator's caller-supplied byte
//!    budget when it is written and the charge is given back when it is dropped, so an
//!    account that writes its feed over and over does not make the store drift.
//!
//! So the tables below span those classes rather than one recorded request: bucket names at
//! each published length bound and one past each, every structural naming rule the guide
//! states, a prefix absent, empty, nested, at its published maximum encoding and one past
//! it in both one-byte and multi-byte characters, a
//! sequence of creates in different orders, and repeated reads of each state.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;

use super::ec2_query::error_code;
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// The `DatafeedSubscriptionState` member a stored feed is in. Pinned against the model's
/// own catalog by [`the_state_a_feed_is_in_is_a_member_of_the_models_catalog`].
const ACTIVE: &str = ec2::state::DATAFEED_SUBSCRIPTION_STATE_ACTIVE;

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call to `action` with `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// The one data feed a response carries, as the four members the service reports on it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Feed {
    bucket: String,
    /// The reported prefix, with an unreported one read as empty: a feed created with no
    /// prefix and one created with an empty prefix both have no prefix to report.
    prefix: String,
    owner_id: String,
    state: String,
    /// Whether the feed carries a `<fault>` at all — "the fault codes for the Spot Instance
    /// request, IF ANY" (`SpotDatafeedSubscription$Fault`).
    has_fault: bool,
}

/// The `<spotDatafeedSubscription>` a response carries, or a panic naming the case.
fn feed(case: &str, body: &str) -> Feed {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "spotDatafeedSubscription")
        .unwrap_or_else(|| {
            panic!("case {case:?}: no <spotDatafeedSubscription> in the response body {body}")
        });
    Feed {
        bucket: child_text(node, "bucket").unwrap_or_default(),
        prefix: child_text(node, "prefix").unwrap_or_default(),
        owner_id: child_text(node, "ownerId").unwrap_or_default(),
        state: child_text(node, "state").unwrap_or_default(),
        has_fault: child(node, "fault").is_some(),
    }
}

/// The form fields of a `CreateSpotDatafeedSubscription` for `bucket` and `prefix`.
fn create_params(bucket: &str, prefix: Option<&str>) -> Vec<Param> {
    let mut params = vec![param("Bucket", bucket)];
    if let Some(prefix) = prefix {
        params.push(param("Prefix", prefix));
    }
    params
}

/// `CreateSpotDatafeedSubscription`, required to succeed; answers the feed it reported.
async fn create(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    prefix: Option<&str>,
) -> Feed {
    let (status, body) = call(
        router,
        "CreateSpotDatafeedSubscription",
        &create_params(bucket, prefix),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 creating the data feed in {bucket:?}, got {status} \
         with body {body}"
    );
    feed(case, &body)
}

/// `DescribeSpotDatafeedSubscription`, required to succeed; answers the feed it reported.
async fn describe(router: &MultiServiceRouter, case: &str) -> Feed {
    let (status, body) = call(router, "DescribeSpotDatafeedSubscription", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSpotDatafeedSubscription, got \
         {status} with body {body}"
    );
    feed(case, &body)
}

/// The feed a create reported, asserted to carry `bucket` and `prefix`, the emulator's own
/// account as its owner, the `Active` state and no fault — and to be exactly what the
/// describe reads back out of the store afterwards.
async fn assert_feed(
    router: &MultiServiceRouter,
    case: &str,
    reported: &Feed,
    bucket: &str,
    prefix: Option<&str>,
) {
    let account = engine::emulator_account_id().to_string();
    let expected = Feed {
        bucket: bucket.to_string(),
        prefix: prefix.unwrap_or_default().to_string(),
        owner_id: account,
        state: ACTIVE.to_string(),
        has_fault: false,
    };
    assert_eq!(
        reported, &expected,
        "case {case:?}: expected the data feed {expected:?}, got {reported:?}"
    );
    // The ROUND TRIP: the reader reports exactly what the writer answered, so the create's
    // response is the stored record and not a value built once for the reply.
    let read_back = describe(router, case).await;
    assert_eq!(
        read_back, expected,
        "case {case:?}: expected the describe to read back the data feed {expected:?}, got \
         {read_back:?}"
    );
}

/// A bucket name of exactly `characters` characters that breaks no OTHER naming rule: all
/// lowercase letters, so a length row differs from its neighbours only in length.
fn bucket_of_length(characters: usize) -> String {
    "a".repeat(characters)
}

// ---------------------------------------------------------------------------
// The catalog the state comes from
// ---------------------------------------------------------------------------

/// The state a stored feed reports is a member of the model's own
/// `DatafeedSubscriptionState` catalog, and so is the word for a feed AWS has stopped
/// delivering — so a catalog the model changes cannot leave this suite asserting a word the
/// service no longer has.
#[tokio::test]
async fn the_state_a_feed_is_in_is_a_member_of_the_models_catalog() {
    let modelled = ec2::ops::types::DATAFEED_SUBSCRIPTION_STATE_VALUES;
    for state in [ACTIVE, ec2::state::DATAFEED_SUBSCRIPTION_STATE_INACTIVE] {
        assert!(
            modelled.contains(&state),
            "case \"DatafeedSubscriptionState catalog\": {state:?} must be a member of the \
             modelled catalog {modelled:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1: one feed per account, created then reported
// ---------------------------------------------------------------------------

/// An account that has created no data feed has none to describe: EC2 refuses the describe
/// with `InvalidSpotDatafeed.NotFound` rather than answering an empty result, and the
/// refusal is repeatable because it wrote nothing.
#[tokio::test]
async fn a_fresh_account_has_no_data_feed_to_describe() {
    let router = emulator();
    for attempt in 1..=3 {
        let case = format!("a fresh account, attempt {attempt}");
        let (status, body) = call(&router, "DescribeSpotDatafeedSubscription", &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a describe with no feed, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_SPOT_DATAFEED_NOT_FOUND,
            "case {case:?}: expected error code {:?}, got {code:?} with body {body}",
            ec2::state::INVALID_SPOT_DATAFEED_NOT_FOUND
        );
    }
}

/// The rule: a create stores the account's one data feed and the describe reads exactly it
/// back.
///
/// The rows span the values the rule generalises over — the bucket name's length at each
/// published bound and its character classes, and the prefix absent, empty, nested and at
/// its published maximum — each against a fresh account, so no row depends on another.
#[tokio::test]
async fn a_created_feed_is_reported_back_by_the_describe() {
    // (case, bucket, prefix)
    let rows: Vec<(String, String, Option<String>)> = vec![
        (
            "the shortest bucket name S3 allows".to_string(),
            "abc".to_string(),
            None,
        ),
        (
            "a bucket name at the published minimum length, with a prefix".to_string(),
            "a-c".to_string(),
            Some("spot/".to_string()),
        ),
        (
            "a bucket name at the published maximum length".to_string(),
            bucket_of_length(ec2::state::MAX_S3_BUCKET_NAME_CHARS),
            Some("feeds/ec2/".to_string()),
        ),
        (
            "a bucket name carrying dots, hyphens and digits".to_string(),
            "spot.feeds-2026.logs".to_string(),
            Some("usage/".to_string()),
        ),
        (
            "a bucket name of dotted digits that is not an IP address".to_string(),
            "10.0.0.4.5".to_string(),
            None,
        ),
        (
            "no prefix at all".to_string(),
            "spot-feed-bucket".to_string(),
            None,
        ),
        (
            "an explicitly empty prefix".to_string(),
            "spot-feed-bucket".to_string(),
            Some(String::new()),
        ),
        (
            "a nested prefix".to_string(),
            "spot-feed-bucket".to_string(),
            Some("a/b/c/d/spotfeed".to_string()),
        ),
        (
            "a prefix at the published maximum length".to_string(),
            "spot-feed-bucket".to_string(),
            Some("p".repeat(ec2::state::MAX_SPOT_DATAFEED_PREFIX_BYTES)),
        ),
        (
            "a multi-byte prefix whose encoding is exactly the published maximum".to_string(),
            "spot-feed-bucket".to_string(),
            // Two bytes per character, so this is 1,024 BYTES and 512 characters: the bound
            // is on the encoding, which is the unit S3 states the key length in.
            Some("\u{e9}".repeat(ec2::state::MAX_SPOT_DATAFEED_PREFIX_BYTES / 2)),
        ),
    ];

    for (case, bucket, prefix) in rows {
        let router = emulator();
        let reported = create(&router, &case, &bucket, prefix.as_deref()).await;
        assert_feed(&router, &case, &reported, &bucket, prefix.as_deref()).await;
    }
}

/// The account holds ONE feed: a create replaces whatever feed the account had, and the
/// describe reports what the LAST create named — never two feeds and never the first one.
///
/// The rows are the sequence's steps, applied in order to one account, so the expectation
/// after each step is that step's own bucket and prefix.
#[tokio::test]
async fn the_account_holds_one_feed_and_the_last_create_wins() {
    let router = emulator();
    // (case, bucket, prefix)
    let steps: [(&str, &str, Option<&str>); 5] = [
        ("the first feed", "spot-feed-one", Some("one/")),
        ("a second create with another bucket", "spot-feed-two", Some("two/")),
        ("the same request again", "spot-feed-two", Some("two/")),
        ("a create that drops the prefix", "spot-feed-two", None),
        ("back to the first bucket", "spot-feed-one", Some("again/")),
    ];

    for (case, bucket, prefix) in steps {
        let reported = create(&router, case, bucket, prefix).await;
        assert_feed(&router, case, &reported, bucket, prefix).await;
    }
}

// ---------------------------------------------------------------------------
// RULE 3: the describe only reports
// ---------------------------------------------------------------------------

/// `DescribeSpotDatafeedSubscription` is a REPORT, so reading cannot change what a later
/// caller sees: a run of describes with no create between them all report the same feed.
///
/// The rows are feeds that differ in what they carry, so the invariant is checked from more
/// than one stored state.
#[tokio::test]
async fn describing_the_feed_never_changes_it() {
    /// Describes per row — enough to catch a read that consumes or advances something.
    const READS: usize = 4;

    // (case, bucket, prefix)
    let rows: [(&str, &str, Option<&str>); 3] = [
        ("a feed with a prefix", "spot-feed-read", Some("logs/")),
        ("a feed with no prefix", "spot-feed-read", None),
        ("a feed in another bucket", "s3.spot-feed.read", Some("x")),
    ];

    for (case, bucket, prefix) in rows {
        let router = emulator();
        let created = create(&router, case, bucket, prefix).await;
        for read in 1..=READS {
            let case = format!("{case}, read {read}");
            let reported = describe(&router, &case).await;
            assert_eq!(
                reported, created,
                "case {case:?}: expected the describe to keep reporting {created:?}, got \
                 {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 2 and the documented refusals
// ---------------------------------------------------------------------------

/// Every refusal these two operations make, each with the EC2 error code and HTTP status it
/// answers — and none of them writes anything.
///
/// The rows differ along what makes the request bad. The bucket rows are the Amazon S3
/// "General purpose bucket naming rules" the `Bucket` member points a caller at, one row per
/// sentence of them — one character short of the published minimum and one past the
/// published maximum, an uppercase letter, an underscore, a leading hyphen, a trailing dot,
/// two adjacent dots, a name formatted as an IP address, and one reserved prefix and one
/// reserved suffix — plus a prefix one character past its maximum encoding, in one-byte and
/// in multi-byte characters. The last two are dry runs
/// of each operation, which "would have succeeded" and so are refused with
/// `DryRunOperation`. Every one of them is a 400 or a 412 — never a 501, which would say the
/// operation is unimplemented.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    // The feed the refusals run against, so "nothing was written" is a claim about a feed
    // that exists rather than about the empty account.
    let (bucket, prefix) = ("spot-feed-seeded", Some("seeded/"));
    let seeded = create(&router, "the refusal table's starting feed", bucket, prefix).await;

    let invalid = ec2::state::INVALID_PARAMETER_VALUE;
    let dry_run = ec2::state::DRY_RUN_OPERATION;
    let too_short = bucket_of_length(ec2::state::MIN_S3_BUCKET_NAME_CHARS - 1);
    let too_long = bucket_of_length(ec2::state::MAX_S3_BUCKET_NAME_CHARS + 1);
    let long_prefix = "p".repeat(ec2::state::MAX_SPOT_DATAFEED_PREFIX_BYTES + 1);
    // 1,026 bytes in 513 characters: past the bound on the ENCODING while well inside the
    // same count of characters, so this row fails only if the bound is measured in bytes.
    let long_multi_byte_prefix =
        "\u{e9}".repeat(ec2::state::MAX_SPOT_DATAFEED_PREFIX_BYTES / 2 + 1);

    // (case, the action, its form fields, the HTTP status, the EC2 error code)
    let cases: Vec<(String, &str, Vec<Param>, u16, &str)> = vec![
        (
            "a bucket name one character short of the published minimum".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params(&too_short, None),
            400,
            invalid,
        ),
        (
            "a bucket name one character past the published maximum".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params(&too_long, None),
            400,
            invalid,
        ),
        (
            "a bucket name carrying an uppercase letter".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params("Spot-Feed", None),
            400,
            invalid,
        ),
        (
            "a bucket name carrying an underscore".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params("spot_feed", None),
            400,
            invalid,
        ),
        (
            "a bucket name beginning with a hyphen".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params("-spot-feed", None),
            400,
            invalid,
        ),
        (
            "a bucket name ending with a dot".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params("spot-feed.", None),
            400,
            invalid,
        ),
        (
            "a bucket name carrying two adjacent dots".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params("spot..feed", None),
            400,
            invalid,
        ),
        (
            "a bucket name formatted as an IP address".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params("192.168.5.4", None),
            400,
            invalid,
        ),
        (
            "a bucket name using one of S3's reserved prefixes".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params(
                &format!("{}spot-feed", ec2::state::S3_BUCKET_NAME_RESERVED_PREFIXES[0]),
                None,
            ),
            400,
            invalid,
        ),
        (
            "a bucket name using one of S3's reserved suffixes".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params(
                &format!("spot-feed{}", ec2::state::S3_BUCKET_NAME_RESERVED_SUFFIXES[0]),
                None,
            ),
            400,
            invalid,
        ),
        (
            "a prefix one character past the published maximum".to_string(),
            "CreateSpotDatafeedSubscription",
            create_params("spot-feed-long-prefix", Some(&long_prefix)),
            400,
            invalid,
        ),
        (
            "a multi-byte prefix one character past the published maximum encoding"
                .to_string(),
            "CreateSpotDatafeedSubscription",
            create_params("spot-feed-long-prefix", Some(&long_multi_byte_prefix)),
            400,
            invalid,
        ),
        (
            "a dry run of a create that would have succeeded".to_string(),
            "CreateSpotDatafeedSubscription",
            {
                let mut params = create_params("spot-feed-dry-run", Some("dry/"));
                params.push(param("DryRun", "true"));
                params
            },
            412,
            dry_run,
        ),
        (
            "a dry run of a describe that would have succeeded".to_string(),
            "DescribeSpotDatafeedSubscription",
            vec![param("DryRun", "true")],
            412,
            dry_run,
        ),
    ];

    for (case, action, params, status_expected, code_expected) in cases {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from {action}, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?} from {action}, got \
             {code:?} with body {body}"
        );
        // Each refusal leaves the feed exactly where it was: the next row starts from the
        // same feed this one did.
        let reported = describe(&router, &case).await;
        assert_eq!(
            reported, seeded,
            "case {case:?}: a refused {action} must leave the feed {seeded:?}, got {reported:?}"
        );
    }
}

/// A `DryRun` that is explicitly off is not a dry run: the request runs normally.
///
/// The rows are the writer and the reader, so the flag's off value is exercised on both
/// operations that read it.
#[tokio::test]
async fn a_dry_run_flag_set_to_false_runs_the_request() {
    let router = emulator();
    let (bucket, prefix) = ("spot-feed-not-a-dry-run", Some("live/"));

    let mut params = create_params(bucket, prefix);
    params.push(param("DryRun", "false"));
    let (status, body) = call(&router, "CreateSpotDatafeedSubscription", &params).await;
    assert_eq!(
        status, 200,
        "case \"a create with DryRun=false\": expected HTTP 200, got {status} with body {body}"
    );
    let created = feed("a create with DryRun=false", &body);

    let (status, body) = call(
        &router,
        "DescribeSpotDatafeedSubscription",
        &[param("DryRun", "false")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a describe with DryRun=false\": expected HTTP 200, got {status} with body \
         {body}"
    );
    let reported = feed("a describe with DryRun=false", &body);
    assert_eq!(
        reported, created,
        "case \"a describe with DryRun=false\": expected the created feed {created:?}, got \
         {reported:?}"
    );
    assert_feed(
        &router,
        "a dry run flag set to false",
        &reported,
        bucket,
        prefix,
    )
    .await;
}

// ---------------------------------------------------------------------------
// RULE 4: the one record's byte charge
// ---------------------------------------------------------------------------

/// The store charges the feed's caller-chosen bytes once and gives them back when the
/// record goes: a create charges exactly
/// [`ec2::state::spot_datafeed_payload_bytes`] for the record it stored, a create that
/// REPLACES the feed charges only the difference, and the removal
/// `DeleteSpotDatafeedSubscription` will use returns the store to exactly what it held
/// before — so repeated writes cannot make the budget drift.
///
/// The rows are prefixes written in sequence onto one account, differing along the length
/// the charge is a function of, so it is exercised growing, staying the same and shrinking.
#[tokio::test]
async fn the_one_feed_is_charged_once_and_its_charge_is_given_back() {
    let (router, engine) = super::ec2_query::emulator();
    let store = engine.state::<Ec2State>();
    // What the seeded default network already holds: the charge is asserted as a delta on
    // it, never as an absolute.
    let before = store.lock().stored_bytes();

    let long = "p".repeat(512);
    // (case, bucket, prefix)
    let steps: Vec<(&str, &str, Option<&str>)> = vec![
        ("a short prefix", "spot-feed-bytes", Some("a/")),
        ("a longer prefix", "spot-feed-bytes", Some(long.as_str())),
        ("the same prefix again", "spot-feed-bytes", Some(long.as_str())),
        ("back to a short prefix", "spot-feed-bytes", Some("a/")),
        ("a longer bucket name and no prefix", "spot-feed-bytes-again", None),
    ];

    for (case, bucket, prefix) in steps {
        create(&router, case, bucket, prefix).await;
        let charged = store.lock().stored_bytes();
        let expected = before + ec2::state::spot_datafeed_payload_bytes(bucket, prefix);
        assert_eq!(
            charged, expected,
            "case {case:?}: expected the store to hold {expected} bytes after the create, \
             got {charged}"
        );
    }

    let removed = store.lock().remove_spot_datafeed_subscription();
    assert!(
        removed,
        "case \"removing the stored feed\": expected the account to have had a feed to remove"
    );
    let released = store.lock().stored_bytes();
    assert_eq!(
        released, before,
        "case \"removing the stored feed\": expected the charge to be given back, leaving \
         {before} bytes, got {released}"
    );

    let removed_again = store.lock().remove_spot_datafeed_subscription();
    assert!(
        !removed_again,
        "case \"removing a feed that is already gone\": expected nothing to remove"
    );
    let still = store.lock().stored_bytes();
    assert_eq!(
        still, before,
        "case \"removing a feed that is already gone\": expected the store to stand at \
         {before} bytes, got {still}"
    );

    // And the account is back to having nothing to describe.
    let (status, body) = call(&router, "DescribeSpotDatafeedSubscription", &[]).await;
    assert_eq!(
        status, 400,
        "case \"describing a removed feed\": expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code,
        ec2::state::INVALID_SPOT_DATAFEED_NOT_FOUND,
        "case \"describing a removed feed\": expected error code {:?}, got {code:?} with \
         body {body}",
        ec2::state::INVALID_SPOT_DATAFEED_NOT_FOUND
    );
}
