// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDeliveries`: what the service reports about the account's
//! deliveries.
//!
//! The rule under test: `DescribeDeliveries` reports, from stored state, every delivery
//! associated with ANY of the delivery source names the request lists (all of them when it
//! lists none), each one carrying all of the model's `Delivery` members as that delivery
//! holds them, enumerated in one stable order and handed back at most `limit` (default 50)
//! per page with a `nextToken` exactly when more remain. Reporting is a read: calling it
//! twice reports the same thing.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.DescribeDeliveries` through the router that mounts it — so
//! what is asserted here is what a client reads off the wire.
//!
//! The deliveries themselves are put in the store through [`DeliveryRecord::new`] — the SAME
//! constructor `CreateDelivery` will call — because that operation is still a scaffold stub
//! (`issues/.../logs-createdelivery-stub.md`), so no wire call can create one yet. Once it is
//! implemented, these suites' seeding is the only thing that changes; nothing they assert
//! does.
//!
//! The table spans what the rule generalises over rather than one request: a destination of
//! each vendor the model's `DeliveryDestinationType` enumerates and one outside it, several
//! sources and one no delivery uses, a filter naming none / one / several / every source,
//! record fields absent / empty / several, a delimiter at both published length bounds, an
//! S3 configuration present and absent, tag maps of zero, one and several entries, and every
//! page size from one delivery to more than the store holds.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::ops::types::{
    S3DeliveryConfiguration, DELIVERY_DESTINATION_TYPE_VALUES,
};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    delivery_arn, ComAmazonawsCloudwatchlogsLogs20140328State, DeliveryRecord,
};

use crate::harness::{Logs, REGION};

/// The operation every case calls.
const OP: &str = "DescribeDeliveries";

/// Three delivery sources the stored deliveries are associated with, and a fourth no stored
/// delivery names — `deliverySourceNames` "filters the results to deliveries associated with
/// any specified delivery source". Each is a `DeliverySourceName` (`^[\w-]*$`, 1..60).
const SOURCE_ONE: &str = "vpc-flow-logs";
const SOURCE_TWO: &str = "waf_logs";
const SOURCE_THREE: &str = "route53-resolver-logs";
const SOURCE_UNUSED: &str = "eks-audit-logs";

/// A delivery destination of each vendor a delivery can deliver to — "The destination can be
/// CloudWatch Logs, Amazon S3, Firehose or X-Ray" — plus one whose ARN names a vendor that is
/// none of them, which therefore has no `DeliveryDestinationType` to report.
const DEST_CWL: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/one:*";
const DEST_CWL_OTHER: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/two:*";
const DEST_S3: &str = "arn:aws:s3:::vended-logs-bucket";
const DEST_FH: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs";
const DEST_XRAY: &str = "arn:aws:xray:eu-west-2:123456789012:segment-destination/vended-traces";
const DEST_OUTSIDE_THE_ENUM: &str = "arn:aws:kinesis:eu-west-2:123456789012:stream/vended-logs";

/// One seeded delivery: the state a reporting call must answer from.
struct Seed {
    /// `Delivery$id` (`^[0-9A-Za-z]+$`), also the key the store enumerates by.
    id: &'static str,
    source: &'static str,
    destination: &'static str,
    /// The `deliveryDestinationType` the destination ARN's vendor implies, or `None` for a
    /// destination outside the model's enum.
    destination_type: Option<&'static str>,
    /// `recordFields`, absent / the empty list / several fields.
    record_fields: Option<Vec<&'static str>>,
    /// `fieldDelimiter`, absent or at either published length bound (`FieldDelimiter` 0..5).
    field_delimiter: Option<&'static str>,
    s3: Option<S3DeliveryConfiguration>,
    /// `tags`, zero / one / several entries.
    tags: Vec<(&'static str, &'static str)>,
}

/// The corpus every filtering, paging and reporting case below reads: six deliveries
/// spanning all four `DeliveryDestinationType` values, one destination outside them, three
/// sources, and each form the optional `Delivery` members take.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            id: "AAA111",
            source: SOURCE_ONE,
            destination: DEST_CWL,
            destination_type: Some("CWL"),
            record_fields: None,
            field_delimiter: None,
            s3: None,
            tags: vec![],
        },
        Seed {
            id: "BBB222",
            source: SOURCE_ONE,
            destination: DEST_S3,
            destination_type: Some("S3"),
            record_fields: Some(vec!["event_timestamp", "log_stream", "message"]),
            // The maximum a `FieldDelimiter` admits: five characters.
            field_delimiter: Some("|||||"),
            s3: Some(S3DeliveryConfiguration {
                suffix_path: Some("AWSLogs/{account-id}/{source-type}".to_string()),
                enable_hive_compatible_path: Some(true),
            }),
            tags: vec![("env", "prod")],
        },
        Seed {
            id: "CCC333",
            source: SOURCE_TWO,
            destination: DEST_FH,
            destination_type: Some("FH"),
            // `RecordFields` is `@length(min: 0)`: the empty list is a list the caller named,
            // which is not the same as naming none.
            record_fields: Some(vec![]),
            // The minimum a `FieldDelimiter` admits: zero characters.
            field_delimiter: Some(""),
            s3: None,
            tags: vec![("env", "test"), ("owner", "platform"), ("tier", "2")],
        },
        Seed {
            id: "DDD444",
            source: SOURCE_TWO,
            destination: DEST_XRAY,
            destination_type: Some("XRAY"),
            record_fields: Some(vec!["@timestamp"]),
            field_delimiter: Some("\t"),
            s3: None,
            tags: vec![],
        },
        Seed {
            id: "EEE555",
            source: SOURCE_THREE,
            destination: DEST_CWL_OTHER,
            destination_type: Some("CWL"),
            record_fields: None,
            field_delimiter: None,
            s3: Some(S3DeliveryConfiguration {
                suffix_path: None,
                enable_hive_compatible_path: Some(false),
            }),
            tags: vec![("env", "dev")],
        },
        Seed {
            id: "FFF666",
            source: SOURCE_THREE,
            destination: DEST_OUTSIDE_THE_ENUM,
            destination_type: None,
            record_fields: None,
            field_delimiter: None,
            s3: None,
            tags: vec![],
        },
    ]
}

/// Every seeded id, in the order the store enumerates them (by delivery id).
fn all_ids() -> Vec<String> {
    let mut ids: Vec<String> = seeds().into_iter().map(|seed| seed.id.to_string()).collect();
    ids.sort();
    ids
}

/// Put [`seeds`] in the shared store.
fn seed_store(logs: &Logs) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    for seed in seeds() {
        let mut record = DeliveryRecord::new(seed.id, REGION, seed.source, seed.destination);
        record.record_fields = seed
            .record_fields
            .map(|fields| fields.into_iter().map(ToString::to_string).collect());
        record.field_delimiter = seed.field_delimiter.map(ToString::to_string);
        record.s3_delivery_configuration = seed.s3;
        record.tags = seed
            .tags
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        store.deliveries.insert(record.id.clone(), record);
    }
}

/// The `deliveries` member of one successful page.
async fn page(logs: &Logs, body: &Value) -> Vec<Value> {
    let response = logs.ok(OP, body).await;
    response
        .get("deliveries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{OP} {body}: the response must carry deliveries, got {response}"))
        .clone()
}

/// The `id`s of one successful page, in the order they were reported.
async fn reported_ids(logs: &Logs, body: &Value) -> Vec<String> {
    page(logs, body)
        .await
        .iter()
        .map(|delivery| {
            delivery
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every reported delivery carries an id, got {delivery}"))
                .to_string()
        })
        .collect()
}

/// Every reported delivery of the unfiltered enumeration, keyed by its id.
async fn reported_by_id(logs: &Logs) -> BTreeMap<String, Value> {
    page(logs, &json!({}))
        .await
        .into_iter()
        .map(|delivery| {
            let id = delivery
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every reported delivery carries an id, got {delivery}"))
                .to_string();
            (id, delivery)
        })
        .collect()
}

/// An account with no deliveries is an empty page, not a refusal and not a missing member:
/// `DescribeDeliveries` "retrieves a list of the deliveries that have been created in the
/// account", and nothing has been.
#[tokio::test]
async fn an_account_with_no_deliveries_reports_an_empty_page() {
    let logs = Logs::new();

    // Unfiltered, filtered by a name, and at a page size: none of them invents a delivery.
    let rows: Vec<(&str, Value)> = vec![
        ("no filter at all", json!({})),
        (
            "a delivery source name",
            json!({"deliverySourceNames": [SOURCE_ONE]}),
        ),
        ("a page size", json!({"limit": 1})),
    ];
    for (name, body) in rows {
        let response = logs.ok(OP, &body).await;
        let deliveries = response
            .get("deliveries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {name:?}: expected deliveries, got {response}"));
        assert!(
            deliveries.is_empty(),
            "case {name:?}: an account with no deliveries reports none, got {deliveries:?}"
        );
        assert!(
            response.get("nextToken").is_none(),
            "case {name:?}: an empty enumeration has no next set, got {response}"
        );
    }
}

/// `deliverySourceNames` selects exactly the stored deliveries associated with ANY name it
/// lists — one name, several, every name, a name no delivery uses, and no filter at all — and
/// reports them in the store's one enumeration order.
#[tokio::test]
async fn the_source_filter_selects_exactly_the_deliveries_associated_with_any_name_listed() {
    let logs = Logs::new();
    seed_store(&logs);

    // 19 names no delivery uses plus one that is used, which is the published maximum of 20
    // (`DeliverySourceNames` is `@length(min: 1, max: 20)`).
    let twenty_names: Vec<String> = (0..19)
        .map(|index| format!("{SOURCE_UNUSED}-{index}"))
        .chain(std::iter::once(SOURCE_TWO.to_string()))
        .collect();

    // (case, request, the delivery ids the rule requires, in order)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no filter at all",
            json!({}),
            vec!["AAA111", "BBB222", "CCC333", "DDD444", "EEE555", "FFF666"],
        ),
        (
            "one source, two deliveries",
            json!({"deliverySourceNames": [SOURCE_ONE]}),
            vec!["AAA111", "BBB222"],
        ),
        (
            "the second source",
            json!({"deliverySourceNames": [SOURCE_TWO]}),
            vec!["CCC333", "DDD444"],
        ),
        (
            "the third source",
            json!({"deliverySourceNames": [SOURCE_THREE]}),
            vec!["EEE555", "FFF666"],
        ),
        (
            "two sources at once",
            json!({"deliverySourceNames": [SOURCE_ONE, SOURCE_THREE]}),
            vec!["AAA111", "BBB222", "EEE555", "FFF666"],
        ),
        (
            "every source, in an order of its own",
            json!({"deliverySourceNames": [SOURCE_THREE, SOURCE_ONE, SOURCE_TWO]}),
            vec!["AAA111", "BBB222", "CCC333", "DDD444", "EEE555", "FFF666"],
        ),
        (
            "the same source named twice",
            json!({"deliverySourceNames": [SOURCE_TWO, SOURCE_TWO]}),
            vec!["CCC333", "DDD444"],
        ),
        (
            "a source no delivery is associated with",
            json!({"deliverySourceNames": [SOURCE_UNUSED]}),
            vec![],
        ),
        (
            "a used source beside an unused one",
            json!({"deliverySourceNames": [SOURCE_UNUSED, SOURCE_ONE]}),
            vec!["AAA111", "BBB222"],
        ),
        (
            "the published maximum of twenty names, one of them used",
            json!({"deliverySourceNames": twenty_names}),
            vec!["CCC333", "DDD444"],
        ),
    ];

    for (name, body, expected) in rows {
        let actual = reported_ids(&logs, &body).await;
        assert_eq!(
            actual, expected,
            "case {name:?}: expected deliveries {expected:?}, got {actual:?}"
        );
    }
}

/// Every member of the model's `Delivery` shape is reported as the delivery holds it: the id
/// and the ARN built for it, the source name, the destination ARN and the type its vendor
/// implies, the record fields and delimiter at each form they take, the S3 configuration when
/// there is one, and the tags only when the delivery carries any.
#[tokio::test]
async fn a_reported_delivery_carries_every_member_from_state() {
    let logs = Logs::new();
    seed_store(&logs);
    let reported = reported_by_id(&logs).await;

    for seed in seeds() {
        let name = seed.id;
        let delivery = reported
            .get(seed.id)
            .unwrap_or_else(|| panic!("case {name:?}: the delivery must be reported, got {reported:?}"));
        let string = |member: &str| delivery.get(member).and_then(Value::as_str);

        assert_eq!(
            string("id"),
            Some(seed.id),
            "case {name:?}: expected id {:?}, got {delivery}",
            seed.id
        );
        // The ARN is built for the delivery, not echoed: a `delivery` resource of this
        // service, in the region the request's credential scope names, under this
        // emulator's account.
        let expected_arn = format!(
            "arn:aws:logs:{REGION}:{}:delivery:{}",
            engine::emulator_account_id(),
            seed.id
        );
        assert_eq!(
            string("arn"),
            Some(expected_arn.as_str()),
            "case {name:?}: expected arn {expected_arn:?}, got {delivery}"
        );
        assert_eq!(
            string("arn").map(ToString::to_string),
            Some(delivery_arn(REGION, seed.id)),
            "case {name:?}: the reported arn must be the one this service builds, got {delivery}"
        );
        assert_eq!(
            string("deliverySourceName"),
            Some(seed.source),
            "case {name:?}: expected deliverySourceName {:?}, got {delivery}",
            seed.source
        );
        assert_eq!(
            string("deliveryDestinationArn"),
            Some(seed.destination),
            "case {name:?}: expected deliveryDestinationArn {:?}, got {delivery}",
            seed.destination
        );
        assert_eq!(
            string("deliveryDestinationType"),
            seed.destination_type,
            "case {name:?}: expected deliveryDestinationType {:?}, got {delivery}",
            seed.destination_type
        );
        // Every type reported is a value of the model's `DeliveryDestinationType` enum.
        if let Some(reported_type) = string("deliveryDestinationType") {
            assert!(
                DELIVERY_DESTINATION_TYPE_VALUES.contains(&reported_type),
                "case {name:?}: {reported_type:?} is not one of \
                 {DELIVERY_DESTINATION_TYPE_VALUES:?}"
            );
        }

        let expected_fields = seed
            .record_fields
            .as_ref()
            .map(|fields| Value::from(fields.clone()));
        assert_eq!(
            delivery.get("recordFields").cloned(),
            expected_fields,
            "case {name:?}: expected recordFields {expected_fields:?}, got {delivery}"
        );
        assert_eq!(
            string("fieldDelimiter"),
            seed.field_delimiter,
            "case {name:?}: expected fieldDelimiter {:?}, got {delivery}",
            seed.field_delimiter
        );

        match &seed.s3 {
            None => assert!(
                delivery.get("s3DeliveryConfiguration").is_none(),
                "case {name:?}: a delivery with no S3 configuration reports no member, \
                 got {delivery}"
            ),
            Some(configuration) => {
                let reported_configuration = delivery
                    .get("s3DeliveryConfiguration")
                    .unwrap_or_else(|| panic!("case {name:?}: expected an s3DeliveryConfiguration, got {delivery}"));
                assert_eq!(
                    reported_configuration
                        .get("suffixPath")
                        .and_then(Value::as_str),
                    configuration.suffix_path.as_deref(),
                    "case {name:?}: expected suffixPath {:?}, got {reported_configuration}",
                    configuration.suffix_path
                );
                assert_eq!(
                    reported_configuration
                        .get("enableHiveCompatiblePath")
                        .and_then(Value::as_bool),
                    configuration.enable_hive_compatible_path,
                    "case {name:?}: expected enableHiveCompatiblePath {:?}, \
                     got {reported_configuration}",
                    configuration.enable_hive_compatible_path
                );
            }
        }

        // `Tags` is `@length(min: 1)`, so a delivery carrying none reports no member at all.
        if seed.tags.is_empty() {
            assert!(
                delivery.get("tags").is_none(),
                "case {name:?}: an untagged delivery reports no tags member, got {delivery}"
            );
        } else {
            let expected_tags = Value::Object(
                seed.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), Value::from(*value)))
                    .collect(),
            );
            assert_eq!(
                delivery.get("tags").cloned(),
                Some(expected_tags.clone()),
                "case {name:?}: expected tags {expected_tags}, got {delivery}"
            );
        }
    }
}

/// Whatever the page size, paging walks the one enumeration exactly once: every page is at
/// most `limit` long, a `nextToken` comes back exactly when deliveries remain, and the pages
/// concatenate to the full enumeration with nothing repeated or skipped — filtered or not.
#[tokio::test]
async fn paging_walks_the_enumeration_once_at_every_page_size() {
    let logs = Logs::new();
    seed_store(&logs);

    // (case, the filter, the enumeration it walks)
    let enumerations: Vec<(&str, Value, Vec<String>)> = vec![
        ("unfiltered", json!({}), all_ids()),
        (
            "filtered to two sources",
            json!({"deliverySourceNames": [SOURCE_ONE, SOURCE_TWO]}),
            vec![
                "AAA111".to_string(),
                "BBB222".to_string(),
                "CCC333".to_string(),
                "DDD444".to_string(),
            ],
        ),
    ];

    for (case, filter, expected) in enumerations {
        // One page size below the enumeration, one at it, one above, and the modelled
        // maximum.
        for limit in [1_i64, 2, 3, 4, 6, 7, 50] {
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut body = filter.clone();
                body["limit"] = json!(limit);
                if let Some(next) = &token {
                    body["nextToken"] = json!(next);
                }
                let response = logs.ok(OP, &body).await;
                let deliveries = response
                    .get("deliveries")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| {
                        panic!("case {case:?} limit {limit}: expected deliveries, got {response}")
                    })
                    .clone();
                assert!(
                    i64::try_from(deliveries.len()).unwrap_or(i64::MAX) <= limit,
                    "case {case:?} limit {limit}: a page holds at most {limit} deliveries, got {}",
                    deliveries.len()
                );
                for delivery in &deliveries {
                    collected.push(
                        delivery
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_else(|| {
                                panic!("case {case:?} limit {limit}: expected an id, got {delivery}")
                            })
                            .to_string(),
                    );
                }
                token = response
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let remaining = expected.len() - collected.len();
                assert_eq!(
                    token.is_some(),
                    remaining > 0,
                    "case {case:?} limit {limit}: after {} of {} deliveries the nextToken \
                     presence must match whether more remain, got {response}",
                    collected.len(),
                    expected.len()
                );
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?} limit {limit}: paging must terminate, got {pages} pages for \
                     {} deliveries",
                    expected.len()
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                collected, expected,
                "case {case:?} limit {limit}: the pages must concatenate to the whole \
                 enumeration, expected {expected:?}, got {collected:?}"
            );
        }
    }
}

/// A `nextToken` is a position within ONE filtered enumeration: continuing a different
/// enumeration with it, or handing over something this service never minted, is refused with
/// the error this operation declares rather than answered from the wrong place.
#[tokio::test]
async fn a_token_from_another_enumeration_is_refused() {
    let logs = Logs::new();
    seed_store(&logs);

    let first = logs.ok(OP, &json!({"limit": 1})).await;
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the first page of six deliveries must carry a nextToken, got {first}"))
        .to_string();

    // The same token continued under a filter the enumeration it was minted for did not
    // carry, and three values no enumeration of this service ever minted.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a token from the unfiltered enumeration, replayed with a source filter",
            json!({"limit": 1, "nextToken": token, "deliverySourceNames": [SOURCE_ONE]}),
        ),
        (
            "a token that is not hex at all",
            json!({"nextToken": "not-a-token"}),
        ),
        (
            "a token that is hex but carries no cursor",
            json!({"nextToken": "0011deadbeef"}),
        ),
    ];
    for (name, body) in rows {
        let code = logs.err(OP, &body).await;
        assert_eq!(
            code, "ValidationException",
            "case {name:?}: expected ValidationException, got {code}"
        );
    }

    // The token itself still works for the enumeration it was minted for.
    let continued = reported_ids(&logs, &json!({"limit": 1, "nextToken": token})).await;
    assert_eq!(
        continued,
        vec!["BBB222".to_string()],
        "case \"the token's own enumeration\": expected the second delivery, got {continued:?}"
    );
}

/// Both published bounds of each paging and filtering constraint reach the operation, and one
/// past either is refused by the modelled constraint. Sources: the model's `DescribeLimit`
/// shape (`@range` 1..50), which `DescribeDeliveries$limit` targets, and `DeliverySourceNames`
/// (`@length` 1..20).
#[tokio::test]
async fn the_published_bounds_are_accepted_and_one_past_each_is_refused() {
    let logs = Logs::new();
    seed_store(&logs);

    for limit in [1, 50] {
        let reported = reported_ids(&logs, &json!({"limit": limit})).await;
        let expected = all_ids()
            .into_iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect::<Vec<_>>();
        assert_eq!(
            reported, expected,
            "case \"limit {limit}\": a value on the published bound is answered, \
             expected {expected:?}, got {reported:?}"
        );
    }

    // (case, limit, the constraint clause the refusal must state)
    let limits: [(i64, &str); 2] = [
        (0, "Member must have value greater than or equal to 1"),
        (51, "Member must have value less than or equal to 50"),
    ];
    for (limit, clause) in limits {
        let (code, message) = logs.refusal(OP, &json!({"limit": limit})).await;
        assert_eq!(
            code, "ValidationException",
            "case \"limit {limit}\": expected ValidationException, got {code} {message:?}"
        );
        let expected = format!(
            "1 validation error detected: Value at 'limit' failed to satisfy constraint: {clause}"
        );
        assert_eq!(
            message, expected,
            "case \"limit {limit}\": expected {expected:?}, got {message:?}"
        );
    }

    // A list one past either published bound of `DeliverySourceNames`: empty, and 21 names.
    let too_many: Vec<String> = (0..21)
        .map(|index| format!("{SOURCE_UNUSED}-{index}"))
        .collect();
    let lists: Vec<(&str, Value)> = vec![
        ("no names at all", json!([])),
        ("one name past the maximum of twenty", json!(too_many)),
    ];
    for (name, names) in lists {
        let code = logs.err(OP, &json!({"deliverySourceNames": names})).await;
        assert_eq!(
            code, "ValidationException",
            "case {name:?}: expected ValidationException, got {code}"
        );
    }
}

/// Reporting is a READ: describing the account's deliveries twice reports the same thing, and
/// leaves the store holding exactly the deliveries it held before.
#[tokio::test]
async fn reporting_changes_nothing_a_later_caller_sees() {
    let logs = Logs::new();
    seed_store(&logs);

    let first = logs.ok(OP, &json!({})).await;
    // A filtered call and a paged walk in between: neither consumes anything.
    let _ = logs
        .ok(OP, &json!({"deliverySourceNames": [SOURCE_ONE]}))
        .await;
    let paged = logs.ok(OP, &json!({"limit": 1})).await;
    let token = paged
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a first page of six deliveries carries a nextToken, got {paged}"))
        .to_string();
    let _ = logs.ok(OP, &json!({"limit": 1, "nextToken": token})).await;
    let second = logs.ok(OP, &json!({})).await;

    assert_eq!(
        second, first,
        "reporting twice must report the same thing, expected {first}, got {second}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored: Vec<String> = state.lock().deliveries.keys().cloned().collect();
    assert_eq!(
        stored,
        all_ids(),
        "the store must still hold every seeded delivery, expected {:?}, got {stored:?}",
        all_ids()
    );
}
