// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeFieldIndexes`: the field indexes the service reports for
//! the log groups one request names.
//!
//! The rule under test: one entry per field index the field index policy in effect for a
//! named log group creates — the policy stored on that group when it holds one, otherwise
//! the account-level `FIELD_INDEX_POLICY` whose `selectionCriteria` select it, and none at
//! all for a group outside the Standard log class, which "support[s] field index policies"
//! alone. Each entry carries the field the policy names, the type the policy gives it (the
//! declared type of a `FieldsV2` entry, `FIELD_INDEX` for a plain `Fields` name), the
//! `CUSTOM` category, and the group's ARN only where the index comes from that group's OWN
//! policy — `FieldIndex$logGroupIdentifier` is where "if this field index appears in an index
//! policy that applies only to a single log group, the ARN of that log group is displayed".
//! An index an account-level policy creates is therefore ONE entry however many of the named
//! groups it applies to. An entry's scan times come from the log data the index was in effect
//! for: the most recent ingestion since its policy was last updated, and the earliest and
//! most recent timestamps of the events whose message carries the indexed field.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span what the rule generalises over (how a document names its fields and
//! with which types, how many it names, where the policy in effect is stored, which log class
//! the group is in, which identifier form names it, which categories the request asks for,
//! and whether a stored event matches an index). The recorded case an issue happens to cite
//! is one row.
//!
//! What `PutIndexPolicy` itself admits — including the field-per-policy quota a document past
//! the bound is refused by — is `tests/all/index_policy_puts.rs`; everything here is read
//! back over the wire.

use std::time::Duration;

use serde_json::{json, Value};

use crate::account_policies::{field_index_document, put_body};
use crate::harness::{base_ms, batch, group_with_stream, now_ms, Logs};

/// The operation every case calls.
const OP: &str = "DescribeFieldIndexes";

/// The account-level policy type that indexes fields, from the model's `PolicyType` enum.
/// Source: `smithy_query(query="shape", shape="PolicyType")`.
const FIELD_INDEX_POLICY: &str = "FIELD_INDEX_POLICY";

/// The category of a field index a field index policy names. Source:
/// `smithy_query(query="shape", shape="IndexCategory")` — the enum's four values are
/// `DEFAULT`, `CUSTOM`, `AUTO` and `INACTIVE` — and `FieldIndex$indexCategory`: "CUSTOM:
/// Fields that you added manually to the field index policy."
const CUSTOM: &str = "CUSTOM";

/// The `IndexType` of a field named in a plain `Fields` array, which gives no type of its
/// own. Source: `smithy_query(query="shape", shape="IndexType")` — the enum's two values are
/// `FACET` and `FIELD_INDEX`.
const FIELD_INDEX: &str = "FIELD_INDEX";

/// The other `IndexType` a `FieldsV2` entry can declare. See [`FIELD_INDEX`].
const FACET: &str = "FACET";

/// How long a case waits before an ingestion or a put whose ORDER against the previous one
/// it asserts, so the two land in different wall-clock milliseconds (every CloudWatch Logs
/// timestamp is in milliseconds).
const CLOCK_GAP: Duration = Duration::from_millis(5);

/// One reported entry as the triple that says WHICH field index it is and how it is
/// indexed: the identifier it is reported under (`None` for an index an account-level policy
/// creates), the field it matches, and its type.
type Reported = (Option<String>, String, String);

/// One `(field, type)` pair a policy document indexes.
type Indexed = (String, String);

/// One row of the document-shape table: the case, the policy document, and the pairs it
/// indexes.
type ShapeRow = (&'static str, String, Vec<Indexed>);

/// One row of the policy-in-effect table: the case, the identifiers one request names, and
/// what it reports.
type EffectRow = (&'static str, Vec<String>, Vec<Reported>);

/// One row of the refusal table: the case, the identifiers one request names, and the
/// `indexCategories` it asks for.
type RefusalRow<'a> = (&'static str, Vec<&'a str>, Option<Vec<&'static str>>);

/// A string member of a reported `FieldIndex`.
fn member(index: &Value, key: &str) -> String {
    index
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a field index must carry {key}, got {index}"))
        .to_string()
}

/// One reported entry as [`Reported`] carries it.
fn reported(index: &Value) -> Reported {
    (
        index
            .get("logGroupIdentifier")
            .and_then(Value::as_str)
            .map(str::to_string),
        member(index, "fieldIndexName"),
        member(index, "type"),
    )
}

/// The field indexes one request reports, asserting the operation answers in one page (it
/// mints no `nextToken`) and that every entry is a `CUSTOM` one, the category a field index
/// policy's fields are in.
async fn described(logs: &Logs, identifiers: &[&str], categories: Option<&[&str]>) -> Vec<Value> {
    let mut body = json!({"logGroupIdentifiers": identifiers});
    if let Some(categories) = categories {
        body["indexCategories"] = json!(categories);
    }
    let page = logs.ok(OP, &body).await;
    assert!(
        page.get("nextToken").is_none(),
        "case {identifiers:?}: {OP} answers in one page, got a nextToken in {page}"
    );
    let indexes = page
        .get("fieldIndexes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {identifiers:?}: {OP} must return fieldIndexes, got {page}"))
        .clone();
    for index in &indexes {
        assert_eq!(
            member(index, "indexCategory"),
            CUSTOM,
            "case {identifiers:?}: a field a policy names is a {CUSTOM} index, got {index}"
        );
    }
    indexes
}

/// The triples one request reports, sorted, so a case compares SETS rather than an order it
/// does not assert.
async fn described_triples(
    logs: &Logs,
    identifiers: &[&str],
    categories: Option<&[&str]>,
) -> Vec<Reported> {
    let mut triples: Vec<Reported> = described(logs, identifiers, categories)
        .await
        .iter()
        .map(reported)
        .collect();
    triples.sort();
    triples
}

/// The one entry reported for `field`, or `None` when no entry matches it.
async fn described_field(logs: &Logs, identifier: &str, field: &str) -> Option<Value> {
    described(logs, &[identifier], None)
        .await
        .into_iter()
        .find(|index| member(index, "fieldIndexName") == field)
}

/// Create one log group, in `class` when a class is given.
async fn create_group(logs: &Logs, name: &str, class: Option<&str>) {
    let mut body = json!({"logGroupName": name});
    if let Some(class) = class {
        body["logGroupClass"] = json!(class);
    }
    logs.ok("CreateLogGroup", &body).await;
}

/// Store a log-group level index policy on a group the way a client does.
async fn put_group_policy(logs: &Logs, group: &str, document: &str) {
    logs.ok(
        "PutIndexPolicy",
        &json!({"logGroupIdentifier": group, "policyDocument": document}),
    )
    .await;
}

/// Put one account-level field index policy naming `fields`, scoped by `criteria` when one
/// is given.
async fn put_account_policy(logs: &Logs, name: &str, fields: &[&str], criteria: Option<&str>) {
    let document = field_index_document(fields);
    logs.ok(
        "PutAccountPolicy",
        &put_body(name, FIELD_INDEX_POLICY, &document, criteria),
    )
    .await;
}

/// An `i64` member of a reported `FieldIndex`, or `None` when the entry does not carry one.
fn stamp(index: &Value, key: &str) -> Option<i64> {
    index.get(key).and_then(Value::as_i64)
}

/// Every field a document names is reported as its own entry, carrying the type the document
/// gives it and the ARN of the group whose own policy the document is — for a document
/// naming its fields as a plain `Fields` array, as a typed `FieldsV2` object, as both, with a
/// dotted field name, and at the per-policy field quota.
#[tokio::test]
async fn every_field_the_policy_in_effect_names_is_reported_with_its_type() {
    const GROUP: &str = "/aws/field-indexes/shapes";

    // The 20-field row is the published per-policy bound — `PutIndexPolicy`: "As many as 20
    // fields can be included in the policy" — so it is the largest document a put admits.
    let quota_fields: Vec<String> = (0..20).map(|index| format!("field{index}")).collect();
    let quota_names: Vec<&str> = quota_fields.iter().map(String::as_str).collect();
    let quota_expected: Vec<Indexed> = quota_fields
        .iter()
        .map(|name| (name.clone(), FIELD_INDEX.to_string()))
        .collect();

    // Each row: the case, the policy document, and the `(field, type)` pairs it indexes.
    let rows: Vec<ShapeRow> = vec![
        (
            "one-field",
            field_index_document(&["requestId"]),
            vec![("requestId".to_string(), FIELD_INDEX.to_string())],
        ),
        (
            "three-fields",
            field_index_document(&["requestId", "sessionId", "userId"]),
            vec![
                ("requestId".to_string(), FIELD_INDEX.to_string()),
                ("sessionId".to_string(), FIELD_INDEX.to_string()),
                ("userId".to_string(), FIELD_INDEX.to_string()),
            ],
        ),
        (
            // The example on `PutIndexPolicy$policyDocument`, which names one field per
            // type.
            "fields-v2-types",
            json!({"FieldsV2": {"RequestId": {"type": FIELD_INDEX}, "APIName": {"type": FACET}}})
                .to_string(),
            vec![
                ("APIName".to_string(), FACET.to_string()),
                ("RequestId".to_string(), FIELD_INDEX.to_string()),
            ],
        ),
        (
            "fields-and-fields-v2",
            json!({"Fields": ["TransactionId"], "FieldsV2": {"StatusCode": {"type": FACET}}})
                .to_string(),
            vec![
                ("StatusCode".to_string(), FACET.to_string()),
                ("TransactionId".to_string(), FIELD_INDEX.to_string()),
            ],
        ),
        (
            "dotted-field-name",
            field_index_document(&["requestContext.requestId"]),
            vec![(
                "requestContext.requestId".to_string(),
                FIELD_INDEX.to_string(),
            )],
        ),
        (
            "at-the-field-quota",
            field_index_document(&quota_names),
            quota_expected,
        ),
    ];

    for (case, document, expected) in rows {
        // One group per row, so what it reports is attributable to that row's document
        // alone.
        let logs = Logs::new();
        create_group(&logs, GROUP, None).await;
        let (_, arn) = logs.group_arns(GROUP).await;
        put_group_policy(&logs, GROUP, &document).await;

        let mut wanted: Vec<Reported> = expected
            .iter()
            .map(|(name, index_type)| (Some(arn.clone()), name.clone(), index_type.clone()))
            .collect();
        wanted.sort();
        let reported = described_triples(&logs, &[GROUP], None).await;
        assert_eq!(
            reported, wanted,
            "case {case}: expected one entry per field the document names, got {reported:?}"
        );

        // Nothing has been ingested while this policy was in effect, so no event matched an
        // index and nothing was scanned for one.
        for index in described(&logs, &[GROUP], None).await {
            for key in ["lastScanTime", "firstEventTime", "lastEventTime"] {
                assert!(
                    stamp(&index, key).is_none(),
                    "case {case}: no event was ingested under this policy, so {key} is unset, got {index}"
                );
            }
        }
    }
}

/// A group holding its own index policy reports that policy's fields, under every identifier
/// form that names it and with its own ARN; a group holding none reports the account-level
/// policy's fields, under no ARN at all, and reports nothing once that policy is gone.
#[tokio::test]
async fn the_policy_in_effect_decides_which_field_indexes_are_reported() {
    const HOLDER: &str = "/aws/field-indexes/holder";
    const OTHER: &str = "app/field-indexes/other";
    const ACCOUNT_POLICY: &str = "applies-to-all";

    let logs = Logs::new();
    create_group(&logs, HOLDER, None).await;
    create_group(&logs, OTHER, None).await;
    let (_, holder_arn) = logs.group_arns(HOLDER).await;

    put_account_policy(&logs, ACCOUNT_POLICY, &["accountField"], None).await;
    put_group_policy(&logs, HOLDER, &field_index_document(&["ownField"])).await;

    let own = (Some(holder_arn.clone()), "ownField".to_string(), FIELD_INDEX.to_string());
    let account = (None, "accountField".to_string(), FIELD_INDEX.to_string());

    // Each row: the case, the identifiers one request names, and what it reports.
    let rows: Vec<EffectRow> = vec![
        (
            "own-policy-by-name",
            vec![HOLDER.to_string()],
            vec![own.clone()],
        ),
        (
            // The ARN form the modelled `LogGroupIdentifier` pattern (`^[\w#+=/:,.@-]*$`)
            // admits: no trailing `:*`, which that pattern has no `*` for.
            "own-policy-by-arn",
            vec![holder_arn.clone()],
            vec![own.clone()],
        ),
        (
            "account-policy-for-a-group-holding-none",
            vec![OTHER.to_string()],
            vec![account.clone()],
        ),
        (
            "both-groups-in-one-request",
            vec![HOLDER.to_string(), OTHER.to_string()],
            vec![account.clone(), own.clone()],
        ),
        (
            // "a group named twice is still one log group": the same group under both of its
            // identifier forms is one group's indexes, not two.
            "one-group-under-two-identifier-forms",
            vec![HOLDER.to_string(), holder_arn.clone()],
            vec![own.clone()],
        ),
    ];

    for (case, identifiers, expected) in rows {
        let names: Vec<&str> = identifiers.iter().map(String::as_str).collect();
        let mut wanted = expected;
        wanted.sort();
        let got = described_triples(&logs, &names, None).await;
        assert_eq!(
            got, wanted,
            "case {case}: expected the indexes of the policy in effect, got {got:?}"
        );
    }

    // Removing the group's own policy leaves the account-level one in effect for it, and
    // removing that one leaves the group with no field index at all.
    logs.ok("DeleteIndexPolicy", &json!({"logGroupIdentifier": HOLDER}))
        .await;
    let got = described_triples(&logs, &[HOLDER], None).await;
    assert_eq!(
        got,
        vec![account.clone()],
        "case own-policy-deleted: the account-level policy is what applies now, got {got:?}"
    );

    logs.ok(
        "DeleteAccountPolicy",
        &json!({"policyName": ACCOUNT_POLICY, "policyType": FIELD_INDEX_POLICY}),
    )
    .await;
    for group in [HOLDER, OTHER] {
        let got = described_triples(&logs, &[group], None).await;
        assert!(
            got.is_empty(),
            "case account-policy-deleted/{group}: no policy applies, got {got:?}"
        );
    }
}

/// An account-level index is ONE entry however many of the named groups its criteria select,
/// and no entry at all for a group they select none of.
#[tokio::test]
async fn an_account_level_index_is_reported_once_for_every_group_it_applies_to() {
    const GROUPS: [&str; 3] = [
        "/aws/field-indexes/one",
        "/aws/field-indexes/two",
        "app/field-indexes/three",
    ];

    // Each row: the case, the criteria the policy is put with, and which of [`GROUPS`] it
    // then applies to.
    let rows: Vec<(&str, Option<&str>, [bool; 3])> = vec![
        ("applies-to-all", None, [true, true, true]),
        (
            "scoped-to-a-prefix",
            Some(r#"LogGroupNamePrefix IN ["/aws/field-indexes"]"#),
            [true, true, false],
        ),
        (
            // A prefix that is a whole log group name selects that group alone.
            "scoped-to-a-whole-name",
            Some(r#"LogGroupNamePrefix IN ["app/field-indexes/three"]"#),
            [false, false, true],
        ),
        (
            "scoped-away-from-a-prefix",
            Some(r#"LogGroupNamePrefix NOT IN ["/aws/field-indexes"]"#),
            [false, false, true],
        ),
        (
            "scoped-to-a-prefix-no-group-starts-with",
            Some(r#"LogGroupNamePrefix IN ["/nowhere"]"#),
            [false, false, false],
        ),
    ];

    for (case, criteria, applies) in rows {
        let logs = Logs::new();
        for group in GROUPS {
            create_group(&logs, group, None).await;
        }
        put_account_policy(&logs, case, &["accountField"], criteria).await;
        let expected = vec![(None, "accountField".to_string(), FIELD_INDEX.to_string())];

        // One request naming every group reports the one index, once, as long as the
        // criteria select any of them.
        let got = described_triples(&logs, &GROUPS, None).await;
        if applies.iter().any(|selected| *selected) {
            assert_eq!(
                got, expected,
                "case {case}: an account-level index is one entry for the groups it applies to, got {got:?}"
            );
        } else {
            assert!(
                got.is_empty(),
                "case {case}: the criteria select none of the named groups, got {got:?}"
            );
        }

        // And each group on its own reports it exactly when the criteria select that group.
        for (group, selected) in GROUPS.iter().zip(applies) {
            let got = described_triples(&logs, &[group], None).await;
            if selected {
                assert_eq!(
                    got, expected,
                    "case {case}/{group}: the criteria select this group, got {got:?}"
                );
            } else {
                assert!(
                    got.is_empty(),
                    "case {case}/{group}: the criteria do not select this group, got {got:?}"
                );
            }
        }
    }
}

/// "Only log groups in the Standard log class support field index policies", so an
/// account-level policy that selects every group in the account still gives a group in any
/// other class no field index.
#[tokio::test]
async fn only_a_standard_class_log_group_has_field_indexes() {
    // Each row: the log class the group is created in (`None` for the default) and whether
    // the account-wide policy's index is reported for it. Source:
    // `smithy_query(query="shape", shape="LogGroupClass")` — the enum's values are
    // `DELIVERY`, `INFREQUENT_ACCESS` and `STANDARD`.
    let rows: Vec<(&str, Option<&str>, bool)> = vec![
        ("default-class", None, true),
        ("standard", Some("STANDARD"), true),
        ("infrequent-access", Some("INFREQUENT_ACCESS"), false),
        ("delivery", Some("DELIVERY"), false),
    ];

    for (case, class, indexed) in rows {
        let logs = Logs::new();
        let group = format!("/aws/field-indexes/{case}");
        create_group(&logs, &group, class).await;
        put_account_policy(&logs, "applies-to-all", &["accountField"], None).await;

        let got = described_triples(&logs, &[&group], None).await;
        if indexed {
            assert_eq!(
                got,
                vec![(None, "accountField".to_string(), FIELD_INDEX.to_string())],
                "case {case}: a Standard class group has the account policy's indexes, got {got:?}"
            );
        } else {
            assert!(
                got.is_empty(),
                "case {case}: this log class supports no field index policy, got {got:?}"
            );
        }
    }
}

/// An entry's scan times come from the log data the index was in effect for: the most recent
/// ingestion since the policy was put, and the earliest and latest timestamps of the events
/// whose message carries the indexed field — matched case-sensitively, through a dotted path,
/// and never in a message that is not a JSON object.
#[tokio::test]
async fn the_scan_times_come_from_the_events_ingested_under_the_policy() {
    const GROUP: &str = "/aws/field-indexes/events";
    const STREAM: &str = "events";

    let logs = Logs::new();
    group_with_stream(&logs, GROUP, STREAM, None).await;

    // An event ingested BEFORE the policy is not one the index was scanned for: an entry's
    // `firstEventTime` is "the earliest log event that matches this field index, AFTER the
    // index policy that contains it was created".
    let base = base_ms();
    let put_events = async |events: Value| {
        logs.ok(
            "PutLogEvents",
            &json!({"logGroupName": GROUP, "logStreamName": STREAM, "logEvents": events}),
        )
        .await
    };
    put_events(batch(&[(base, r#"{"requestId":"before-the-policy"}"#)])).await;
    tokio::time::sleep(CLOCK_GAP).await;

    put_group_policy(
        &logs,
        GROUP,
        &field_index_document(&["requestId", "requestContext.requestId", "neverIngested"]),
    )
    .await;

    // Nothing has been ingested since, so nothing was scanned for these indexes.
    for index in described(&logs, &[GROUP], None).await {
        assert!(
            stamp(&index, "lastScanTime").is_none(),
            "case before-ingestion: nothing was scanned under this policy yet, got {index}"
        );
        assert!(
            stamp(&index, "firstEventTime").is_none(),
            "case before-ingestion: the event ingested before the policy is not this index's, got {index}"
        );
    }

    tokio::time::sleep(CLOCK_GAP).await;
    let ingested_at = now_ms();
    put_events(batch(&[
        (base + 1_000, r#"{"requestId":"a"}"#),
        (
            base + 2_000,
            r#"{"requestId":"b","requestContext":{"requestId":"nested"}}"#,
        ),
        (base + 3_000, "a plain text line carrying no field at all"),
        // "Matches of log events to the names of indexed fields are case-sensitive. For
        // example, a field index of RequestId won't match a log event containing requestId."
        (base + 4_000, r#"{"RequestId":"other-case"}"#),
    ]))
    .await;

    // Each row: the indexed field and the `(firstEventTime, lastEventTime)` the events
    // matching it bound.
    let rows: Vec<(&str, Option<(i64, i64)>)> = vec![
        ("requestId", Some((base + 1_000, base + 2_000))),
        (
            "requestContext.requestId",
            Some((base + 2_000, base + 2_000)),
        ),
        ("neverIngested", None),
    ];

    for (field, window) in rows {
        let index = described_field(&logs, GROUP, field)
            .await
            .unwrap_or_else(|| panic!("case {field}: the policy names this field index"));
        // Every event taken was scanned for every index in effect, so each entry reports the
        // ingestion that scanned it.
        let scanned = stamp(&index, "lastScanTime")
            .unwrap_or_else(|| panic!("case {field}: events were ingested, got {index}"));
        assert!(
            (ingested_at..=now_ms()).contains(&scanned),
            "case {field}: expected a scan time in [{ingested_at}, now], got {scanned}"
        );
        match window {
            Some((first, last)) => {
                assert_eq!(
                    stamp(&index, "firstEventTime"),
                    Some(first),
                    "case {field}: expected the earliest matching event, got {index}"
                );
                assert_eq!(
                    stamp(&index, "lastEventTime"),
                    Some(last),
                    "case {field}: expected the most recent matching event, got {index}"
                );
            }
            None => {
                for key in ["firstEventTime", "lastEventTime"] {
                    assert!(
                        stamp(&index, key).is_none(),
                        "case {field}: no ingested event carries this field, so {key} is unset, got {index}"
                    );
                }
            }
        }
    }
}

/// "The index categories to return": a request naming categories reports the field index
/// policy's fields exactly when it asks for the `CUSTOM` ones, and a request naming none
/// reports them because the default categories include `CUSTOM`.
#[tokio::test]
async fn index_categories_restrict_which_indexes_are_reported() {
    const GROUP: &str = "/aws/field-indexes/categories";

    let logs = Logs::new();
    create_group(&logs, GROUP, None).await;
    let (_, arn) = logs.group_arns(GROUP).await;
    put_group_policy(&logs, GROUP, &field_index_document(&["requestId"])).await;
    let expected = vec![(Some(arn), "requestId".to_string(), FIELD_INDEX.to_string())];

    // Each row: the case, the `indexCategories` the request names, and whether the policy's
    // own field is reported. The default — "By default, the response includes the DEFAULT,
    // CUSTOM, and INACTIVE index categories" — includes CUSTOM, and so does an empty list,
    // which names no category to narrow to.
    let rows: Vec<(&str, Option<Vec<&str>>, bool)> = vec![
        ("no-categories", None, true),
        ("empty-list", Some(vec![]), true),
        ("custom-alone", Some(vec![CUSTOM]), true),
        ("custom-among-others", Some(vec!["DEFAULT", CUSTOM]), true),
        ("default-alone", Some(vec!["DEFAULT"]), false),
        ("auto-and-inactive", Some(vec!["AUTO", "INACTIVE"]), false),
        (
            "every-category-but-custom",
            Some(vec!["DEFAULT", "AUTO", "INACTIVE"]),
            false,
        ),
    ];

    for (case, categories, reported) in rows {
        let got = described_triples(&logs, &[GROUP], categories.as_deref()).await;
        if reported {
            assert_eq!(
                got, expected,
                "case {case}: these categories include the policy's fields, got {got:?}"
            );
        } else {
            assert!(
                got.is_empty(),
                "case {case}: a field a policy names is a {CUSTOM} index, got {got:?}"
            );
        }
    }
}

/// A log group the account does not hold is a missing resource — under every identifier form,
/// and whichever categories the request asks for — and a `nextToken` this operation never
/// minted is an invalid parameter.
#[tokio::test]
async fn a_request_the_store_cannot_answer_is_refused() {
    const GROUP: &str = "/aws/field-indexes/present";
    const ABSENT: &str = "/aws/field-indexes/absent";

    let logs = Logs::new();
    create_group(&logs, GROUP, None).await;
    let (_, arn) = logs.group_arns(GROUP).await;
    let absent_arn = arn.replace("present", "absent");
    put_group_policy(&logs, GROUP, &field_index_document(&["requestId"])).await;

    // Each row: the case and the identifiers one request names — the absent group alone,
    // beside a group the account does hold, and by its ARN.
    let rows: Vec<RefusalRow> = vec![
        ("absent-by-name", vec![ABSENT], None),
        ("absent-by-arn", vec![&absent_arn], None),
        ("absent-beside-a-present-group", vec![GROUP, ABSENT], None),
        ("absent-first", vec![ABSENT, GROUP], None),
        // The refusal does not depend on which categories were asked for: the group is
        // resolved before anything is reported for it.
        (
            "absent-with-categories-naming-no-custom",
            vec![ABSENT],
            Some(vec!["DEFAULT"]),
        ),
    ];

    for (case, identifiers, categories) in rows {
        let mut body = json!({"logGroupIdentifiers": identifiers});
        if let Some(categories) = categories {
            body["indexCategories"] = json!(categories);
        }
        let code = logs.err(OP, &body).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: a log group the account does not hold is a missing resource, got {code}"
        );
    }

    for (case, token) in [("not-a-token", "not-a-token"), ("hex-token", "deadbeef")] {
        let (code, message) = logs
            .refusal(
                OP,
                &json!({"logGroupIdentifiers": [GROUP], "nextToken": token}),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case}: a token this operation never minted is an invalid parameter, got {code}"
        );
        assert!(
            message.contains(token),
            "case {case}: the refusal names the token it was given, got {message}"
        );
    }
}
