// SPDX-License-Identifier: CC0-1.0

//! The instant a mutation records, end to end over HTTP — and how one is COMPARED.
//!
//! Every CloudFront record that carries a `LastModifiedTime` states the same rule about it:
//! the mutation that writes the record records the instant it wrote, every later read of
//! that record reports that same instant, and the next mutation of it records an instant no
//! earlier than the one it replaced. The rule is about the INSTANT, not about the text the
//! instant is serialised to: a restXml `timestamp` travels as RFC-3339 at UTC carrying only
//! the SIGNIFICANT digits of its subsecond fraction (`…T08:00:00.5Z`, `…T08:00:00.25Z`,
//! and, when the fraction is zero, `…T08:00:00Z` with no fraction at all), so one wire
//! instant sorts before another as TEXT in cases where it comes after it in time —
//! `…:00.39091Z` is LATER than `…:00.390903Z` but sorts before it, and `…:00Z` is EARLIER
//! than `…:00.5Z` but sorts after it. An ordering assertion on an instant this service
//! reports therefore goes through [`crate::public_key_lifecycle::wire_instant`], which
//! parses the wire form, and never through a comparison of the two strings.
//!
//! The rows span what the rule generalises over: the resource FAMILY (a key group, a cache
//! policy, an origin request policy and a key value store — four surfaces whose records
//! carry a `LastModifiedTime` and whose update is implemented), what the record is
//! addressed by (a minted id, and a name the create submitted), whether the record also
//! carries a status the clock owes a transition, the configuration each record is created
//! with and updated to, and the LENGTH of the chain of mutations (the create, an update,
//! and an update of that update, so the rule is asserted between every neighbouring pair).
//! The wire form's own ordering is pinned on the fractions a serialised instant can carry,
//! including the zero fraction the serializer writes no digits for.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s and every document comes from
//! the owning family's suite, reused rather than copied; the create, read and update calls
//! below are written once against the collection segment, since all four families bind the
//! same routes under it.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_lifecycle::policy_table as cache_policies;
use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::key_group_lifecycle::{mint_public_keys, Group};
use crate::key_value_store_lifecycle::{create_table as key_value_stores, escape};
use crate::origin_request_policy_lifecycle::policy_table as origin_request_policies;
use crate::public_key_lifecycle::{is_wire_timestamp, wire_instant};

// ---------------------------------------------------------------------------
// Calling the surfaces under test
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/{segment}` with the request document `body`.
async fn create(router: &MultiServiceRouter, segment: &str, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/{segment}"),
        Some(body),
        None,
    )
    .await
}

/// `PUT /2020-05-31/{segment}/{id}` with the request document `body` under the version
/// token `if_match`.
async fn update(
    router: &MultiServiceRouter,
    segment: &str,
    id: &str,
    body: &str,
    if_match: &str,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/{segment}/{id}"),
        Some(body),
        Some(if_match),
    )
    .await
}

/// `GET /2020-05-31/{segment}/{id}`.
async fn read(router: &MultiServiceRouter, segment: &str, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/{segment}/{id}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The records under test
// ---------------------------------------------------------------------------

/// One record to drive through a create and two updates: the collection it lives in, the
/// member of the created record its own routes address it by, and the three documents it is
/// created with and updated to in turn.
#[derive(Debug)]
struct Record {
    /// What this row is a case of, named in every assertion it drives.
    case: String,
    /// The collection segment every route for this family is under.
    segment: &'static str,
    /// The member of the created record that names it in a route — a minted `Id` for most
    /// families, the submitted `Name` for a key value store.
    identifier: &'static str,
    /// The document the create submits, then the two an update submits.
    documents: [String; 3],
}

/// The records every case below is run over: one per resource family, each created with one
/// document and then updated twice, so each mutation really replaces what the previous one
/// stored.
async fn records(router: &MultiServiceRouter) -> Vec<Record> {
    let keys = mint_public_keys(router, "wire-instant", 3).await;
    let group = |case: &str, items: &[String], comment: Option<&str>| Group {
        case: case.to_string(),
        name: "wire-instant-group".to_string(),
        items: items.to_vec(),
        comment: comment.map(str::to_string),
    };
    // A key group keeps its own name across an update (a name identifies ONE group), so the
    // three configurations differ along the key list and the comment.
    let groups = [
        group("one key, no comment", &keys[..1], None),
        group("three keys and a comment", &keys, Some("every key")),
        group("two keys, empty comment", &keys[..2], Some("")),
    ];

    let mut records = vec![Record {
        case: "a key group".to_string(),
        segment: "key-group",
        identifier: "Id",
        documents: [groups[0].xml(), groups[1].xml(), groups[2].xml()],
    }];

    // The two policy families: each configuration comes from the owning suite's table,
    // renamed onto the one record so the update is not refused as a name conflict.
    let cache: Vec<String> = cache_policies()
        .into_iter()
        .take(3)
        .map(|policy| {
            crate::cache_policy_lifecycle::Policy {
                name: "wire-instant-cache".to_string(),
                ..policy
            }
            .xml()
        })
        .collect();
    let origin_request: Vec<String> = origin_request_policies()
        .into_iter()
        .take(3)
        .map(|policy| {
            crate::origin_request_policy_lifecycle::Policy {
                name: "wire-instant-origin-request".to_string(),
                ..policy
            }
            .xml()
        })
        .collect();
    for (case, segment, documents) in [
        ("a cache policy", "cache-policy", cache),
        (
            "an origin request policy",
            "origin-request-policy",
            origin_request,
        ),
    ] {
        let [first, second, third] = <[String; 3]>::try_from(documents).unwrap_or_else(|rows| {
            panic!(
                "case {case}: expected three configurations, got {}",
                rows.len()
            )
        });
        records.push(Record {
            case: case.to_string(),
            segment,
            identifier: "Id",
            documents: [first, second, third],
        });
    }

    // A key value store: addressed by the name its create submitted rather than by a minted
    // id, carrying a status the clock owes a transition, and updated through a request
    // document of its own (`Comment` alone) rather than through the create's.
    let store = key_value_stores()
        .into_iter()
        .next()
        .expect("the key value store table carries a row");
    let comment = |text: &str| {
        format!(
            "<UpdateKeyValueStoreRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Comment>{}</Comment></UpdateKeyValueStoreRequest>",
            escape(text)
        )
    };
    records.push(Record {
        case: "a key value store".to_string(),
        segment: "key-value-store",
        identifier: "Name",
        documents: [
            store.xml(),
            comment("flags & limits"),
            // The empty comment, which an update may land as well as any other text.
            comment(""),
        ],
    });
    records
}

/// The `LastModifiedTime` an answer reports, asserted to be a wire instant.
fn reported_instant(answer: &Answer, case: &str, context: &str) -> String {
    let reported = answer
        .root()
        .find_text("LastModifiedTime")
        .unwrap_or_default()
        .to_string();
    assert!(
        is_wire_timestamp(&reported),
        "case {case} ({context}): expected an ISO-8601 LastModifiedTime, got {reported:?} in {}",
        answer.raw
    );
    reported
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// Each mutation records the instant it wrote, every later read of the record reports that
/// same instant, and the instant a mutation records is no earlier — AS AN INSTANT — than the
/// one it replaced.
#[tokio::test]
async fn a_mutation_records_an_instant_no_earlier_than_the_one_it_replaced() {
    let router = emulator();
    for record in records(&router).await {
        let case = &record.case;
        let segment = record.segment;

        let created = create(&router, segment, &record.documents[0]).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            created.status, created.raw
        );
        let id = created
            .root()
            .text_at(record.identifier)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the create reports no {}: {}",
                    record.identifier, created.raw
                )
            })
            .to_string();
        let mut etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a minted version token in ETag, got {etag:?}"
        );
        let mut previous = reported_instant(&created, case, "the create");

        // The read answers from the stored record, so it reports the create's instant
        // unchanged — the same TEXT, since it is the same instant.
        let fetched = read(&router, segment, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected HTTP 200 reading the created record, got {} with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            reported_instant(&fetched, case, "reading the created record"),
            previous,
            "case {case}: expected the read to report the create's instant {previous}, got {}",
            fetched.raw
        );

        for (step, document) in record.documents[1..].iter().enumerate() {
            let answered = update(&router, segment, &id, document, &etag).await;
            assert_eq!(
                answered.status, 200,
                "case {case} (update {step}): expected HTTP 200 from the update, got {} with {}",
                answered.status, answered.raw
            );
            let written = reported_instant(&answered, case, "the update");
            // The ordering is decided on the INSTANT each text names: the wire form carries
            // only the significant digits of its fraction, so the two strings do not sort
            // the way the instants they name do.
            let (written_at, previous_at) =
                (wire_instant(&written, case), wire_instant(&previous, case));
            assert!(
                written_at >= previous_at,
                "case {case} (update {step}): expected the instant not to move backwards from \
                 {previous} ({previous_at:?}), got {written} ({written_at:?})"
            );
            let minted = answered.header("etag").to_string();
            assert!(
                is_version_token(&minted),
                "case {case} (update {step}): expected a minted version token in ETag, got \
                 {minted:?}"
            );
            assert_ne!(
                minted, etag,
                "case {case} (update {step}): expected a fresh version token, got {etag} again"
            );

            let fetched = read(&router, segment, &id).await;
            assert_eq!(
                fetched.status, 200,
                "case {case} (update {step}): expected HTTP 200 reading the updated record, got \
                 {} with {}",
                fetched.status, fetched.raw
            );
            assert_eq!(
                reported_instant(&fetched, case, "reading the updated record"),
                written,
                "case {case} (update {step}): expected the read to report the instant the update \
                 wrote ({written}), got {}",
                fetched.raw
            );

            previous = written;
            etag = minted;
        }
    }
}

/// The wire form a `timestamp` travels in is ordered by the INSTANT it names, which is what
/// [`wire_instant`] reads out of it — and NOT by the text, which carries only the
/// significant digits of the subsecond fraction and so sorts two instants the other way
/// round.
#[test]
fn a_wire_instant_is_ordered_by_its_instant_and_not_by_its_text() {
    // One whole second, in the fractions an instant's subsecond part can land on. Each is
    // exactly representable, so what is rendered below is the serializer's own doing and
    // not a rounding of the literal. The renderings are the restXml `timestamp` wire form
    // (`aws_smithy_types::date_time::Format::DateTime`, the format
    // `vendor/smithy-schema-server`'s XML serializer writes a timestamp with).
    let fractions = [0.0_f64, 0.0625, 0.125, 0.25, 0.5, 0.75];
    let rendered: Vec<String> = fractions
        .iter()
        .map(|fraction| {
            DateTime::from_fractional_secs(1_800_000_000, *fraction)
                .fmt(Format::DateTime)
                .unwrap_or_else(|error| panic!("case {fraction}: unformattable instant: {error}"))
        })
        .collect();

    // The zero fraction is written with no fractional part at all, and a non-zero one with
    // its significant digits only — which is why text order is not instant order.
    assert!(
        !rendered[0].contains('.') && rendered[0].ends_with("00:00Z"),
        "case the zero fraction: expected no fractional part, got {:?}",
        rendered[0]
    );
    assert!(
        rendered[4].ends_with("00:00.5Z"),
        "case the half second: expected a single fractional digit, got {:?}",
        rendered[4]
    );

    // Every pair, in both directions: the parsed instants order exactly as the fractions
    // do, whatever the two strings do.
    let mut disagreements = 0;
    for (left, left_text) in fractions.iter().zip(&rendered) {
        for (right, right_text) in fractions.iter().zip(&rendered) {
            let case = format!("{left_text} against {right_text}");
            let (left_at, right_at) = (
                wire_instant(left_text, &case),
                wire_instant(right_text, &case),
            );
            assert_eq!(
                left_at.cmp(&right_at),
                left.partial_cmp(right).expect("two real fractions"),
                "case {case}: expected the parsed instants to order as the fractions {left} and \
                 {right} do, got {left_at:?} against {right_at:?}"
            );
            if left_text.cmp(right_text) != left_at.cmp(&right_at) {
                disagreements += 1;
            }
        }
    }
    // And the premise itself: comparing the two strings really does disagree with the
    // instants they name, so an ordering assertion on a reported instant cannot be a string
    // comparison.
    assert!(
        disagreements > 0,
        "case the wire form: expected text order to disagree with instant order on at least one \
         pair of {rendered:?}, got none"
    );
}
