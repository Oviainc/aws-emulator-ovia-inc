// SPDX-License-Identifier: CC0-1.0

//! The members a successful Glue answer carries, for the two classes of answer whose
//! members are computed rather than echoed.
//!
//! RULE 1 — a mutation that answers with a resource projection reports the STORED record's
//! whole identity, not just the members the request echoed: the identifier the service
//! assigned the resource, the associations only the create could set, and the instants it
//! was created and last modified. The caller must be able to address and order the resource
//! from the answer alone, without re-reading it.
//!
//! RULE 2 — a listing's collection member is PRESENT in every successful answer, as an
//! empty list when there is nothing to report: an empty listing is an empty collection, not
//! an omitted member, so an SDK paginator iterating the member never meets a null. The
//! continuation token is the only member a listing omits, and it is omitted exactly when no
//! further page follows.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows of each table differ along what its rule generalises over: for rule 1, which
//! members the stored record carries and the form and size of the ones the patch names; for
//! rule 2, which listing is asked, with which cursor.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::wire::{emulator, ok, ACCOUNT_ID};

/// The members `UpdateDataQualityRuleset` answers with: the three the published shape binds,
/// plus the four the service also reports (the ruleset's identifier, its target table and
/// its two timestamps).
const UPDATE_RULESET_RESPONSE_MEMBERS: [&str; 7] = [
    "Name",
    "Description",
    "Ruleset",
    "Identifier",
    "TargetTable",
    "CreatedOn",
    "LastModifiedOn",
];

/// The longest a Glue `HashString` id may be, which is what bounds the identifier the
/// service assigns a ruleset: `HashString`, "len 1..255"
/// (`smithy_query shape=GetDataQualityRulesetResponse`, member `RecommendationRunId`).
const MAX_HASH_STRING_LENGTH: usize = 255;

/// The string member `member` of an answer, or `None` when the answer does not carry it.
fn member<'a>(body: &'a Value, member: &str) -> Option<&'a str> {
    body.get(member).and_then(Value::as_str)
}

/// The timestamp member `member` of an answer, as seconds since the epoch.
fn instant(case: &str, member: &str, body: &Value) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: {member} must be a timestamp, got {body}"))
}

// ---------------------------------------------------------------------------
// RULE 1: an update answers with the stored record's whole identity
// ---------------------------------------------------------------------------

/// Every `UpdateDataQualityRuleset` answers with the ruleset as the store now holds it: the
/// patched definition AND the ruleset's identity — its identifier, the table it targets and
/// both of its timestamps — each taken from the stored record, so the answer agrees member
/// for member with a `GetDataQualityRuleset` of the same name.
#[tokio::test]
async fn an_update_answers_with_the_stored_rulesets_whole_identity() {
    let router = emulator();

    // (case, the create, the patch)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            // The shape a lifecycle client sends: a ruleset created against a table, then
            // patched in both of its optional members.
            "a ruleset targeting a table, patched in both members",
            json!({
                "Name": "orders-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"order_id\" ]",
                "Description": "the first description",
                "TargetTable": { "DatabaseName": "sales", "TableName": "orders" },
            }),
            json!({
                "Name": "orders-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"order_id\", RowCount > 0 ]",
                "Description": "the second description",
            }),
        ),
        (
            // `DataQualityTargetTable` carries an optional `CatalogId` beside the two
            // required names, so a ruleset addressed at a catalog reports all three.
            "a ruleset targeting a table in a named catalog",
            json!({
                "Name": "catalogued-ruleset",
                "Ruleset": "Rules = [ IsComplete \"id\" ]",
                "TargetTable": {
                    "DatabaseName": "sales",
                    "TableName": "customers",
                    "CatalogId": ACCOUNT_ID,
                },
                "DataQualitySecurityConfiguration": "dq-security-config",
                "Tags": { "owner": "data-governance" },
            }),
            json!({ "Name": "catalogued-ruleset", "Description": "added by the update" }),
        ),
        (
            // No target table at all: the member the create never set is reported absent
            // rather than invented.
            "a ruleset targeting no table, patched in the ruleset alone",
            json!({
                "Name": "untargeted-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"a\" ]",
            }),
            json!({ "Name": "untargeted-ruleset", "Ruleset": "Rules = [ RowCount > 1 ]" }),
        ),
        (
            // `DescriptionString` has `length 0..2048`: the empty string is a description a
            // patch may name, and a 2048-byte one sits on the published bound.
            "a patch naming the empty description",
            json!({
                "Name": "empty-description",
                "Ruleset": "Rules = [ ColumnExists \"b\" ]",
                "Description": "replaced by the empty string",
                "TargetTable": { "DatabaseName": "db", "TableName": "t" },
            }),
            json!({ "Name": "empty-description", "Description": "" }),
        ),
        (
            "a patch naming a description at the published upper bound",
            json!({
                "Name": "longest-description",
                "Ruleset": "Rules = [ ColumnExists \"c\" ]",
            }),
            json!({ "Name": "longest-description", "Description": "d".repeat(2_048) }),
        ),
        (
            // `NameString`'s pattern admits a space and a tab, so a name carrying them
            // addresses its ruleset like any other.
            "a name carrying a space and a tab",
            json!({
                "Name": "ruleset one\ttwo",
                "Ruleset": "Rules = [ ColumnExists \"d\" ]",
                "TargetTable": { "DatabaseName": "db two", "TableName": "table\tthree" },
            }),
            json!({ "Name": "ruleset one\ttwo", "Ruleset": "Rules = [ RowCount > 2 ]" }),
        ),
        (
            // A patch that names NO member to change still answers with the whole stored
            // projection: reporting the record is not conditional on writing to it.
            "a patch naming no member to change",
            json!({
                "Name": "untouched-ruleset",
                "Ruleset": "Rules = [ ColumnExists \"e\" ]",
                "Description": "untouched",
                "TargetTable": { "DatabaseName": "db", "TableName": "t" },
            }),
            json!({ "Name": "untouched-ruleset" }),
        ),
    ];

    let mut identifiers: BTreeSet<String> = BTreeSet::new();

    for (case, create, patch) in &rows {
        let name = create["Name"].as_str().expect("the row carries a Name");
        ok(&router, "CreateDataQualityRuleset", create).await;
        let before = ok(&router, "GetDataQualityRuleset", &json!({ "Name": name })).await;

        let answered = ok(&router, "UpdateDataQualityRuleset", patch).await;
        let read = ok(&router, "GetDataQualityRuleset", &json!({ "Name": name })).await;

        // The identity the client addresses the resource by: present, within the bound the
        // model gives a Glue id, and its own per ruleset.
        let identifier = member(&answered, "Identifier").unwrap_or_else(|| {
            panic!("case {case}: the answer must report the ruleset's Identifier, got {answered}")
        });
        assert!(
            !identifier.is_empty() && identifier.len() <= MAX_HASH_STRING_LENGTH,
            "case {case}: expected an identifier of 1..{MAX_HASH_STRING_LENGTH} characters, \
             got {identifier:?}"
        );
        assert!(
            identifiers.insert(identifier.to_string()),
            "case {case}: two rulesets must not share an identifier, got {identifier:?} again"
        );

        // The association only the create could set is reported as the create named it,
        // and is absent for a ruleset created against no table.
        assert_eq!(
            answered.get("TargetTable").cloned().unwrap_or(Value::Null),
            create.get("TargetTable").cloned().unwrap_or(Value::Null),
            "case {case}: expected the TargetTable the create named, got {answered}"
        );

        // The instants: the creation one is the one the read reports (an update never moves
        // it), and the modification one is the record's as it now stands.
        assert_eq!(
            instant(case, "CreatedOn", &answered),
            instant(case, "CreatedOn", &before),
            "case {case}: an update reports the creation instant it was created with, got \
             {answered}"
        );
        assert_eq!(
            instant(case, "LastModifiedOn", &answered),
            instant(case, "LastModifiedOn", &read),
            "case {case}: the answer's LastModifiedOn must be the record's, got {answered} \
             then {read}"
        );

        // The answer agrees with the read, member for member, over every member the two
        // shapes share — so a client that caches the answer holds what a re-read would give.
        for shared in [
            "Name",
            "Description",
            "Ruleset",
            "TargetTable",
            "CreatedOn",
            "LastModifiedOn",
        ] {
            assert_eq!(
                answered.get(shared).cloned().unwrap_or(Value::Null),
                read.get(shared).cloned().unwrap_or(Value::Null),
                "case {case}: the update and the read must agree about {shared}; update gave \
                 {answered}, read gave {read}"
            );
        }

        // And nothing beyond the members this answer carries.
        for reported in answered
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: the answer is a structure, got {answered}"))
            .keys()
        {
            assert!(
                UPDATE_RULESET_RESPONSE_MEMBERS.contains(&reported.as_str()),
                "case {case}: the answer must carry only {UPDATE_RULESET_RESPONSE_MEMBERS:?}, \
                 got {reported} in {answered}"
            );
        }
    }
}

/// The identifier is the ruleset's own and does not move: successive updates of one ruleset
/// all report the identifier its create assigned it, while a ruleset deleted and created
/// again under the same name is a new record and reports a new one.
#[tokio::test]
async fn a_rulesets_identifier_is_stable_across_updates() {
    let router = emulator();
    ok(
        &router,
        "CreateDataQualityRuleset",
        &json!({ "Name": "stable", "Ruleset": "Rules = [ ColumnExists \"a\" ]" }),
    )
    .await;

    // Each row is another patch over the same ruleset, differing in which member it names.
    let patches = [
        json!({ "Name": "stable", "Ruleset": "Rules = [ RowCount > 0 ]" }),
        json!({ "Name": "stable", "Description": "the second description" }),
        json!({ "Name": "stable" }),
        json!({
            "Name": "stable",
            "Description": "the third description",
            "Ruleset": "Rules = [ RowCount > 1 ]",
        }),
    ];

    let mut first: Option<String> = None;
    for (index, patch) in patches.iter().enumerate() {
        let answered = ok(&router, "UpdateDataQualityRuleset", patch).await;
        let reported = member(&answered, "Identifier")
            .unwrap_or_else(|| panic!("case patch {index}: expected an Identifier, got {answered}"))
            .to_string();
        match &first {
            None => first = Some(reported),
            Some(expected) => assert_eq!(
                &reported, expected,
                "case patch {index}: an update must report the identifier the create assigned; \
                 expected {expected:?}, got {reported:?}"
            ),
        }
    }

    // A fresh record under the same name is a different ruleset, so it carries a different
    // identifier.
    ok(
        &router,
        "DeleteDataQualityRuleset",
        &json!({ "Name": "stable" }),
    )
    .await;
    ok(
        &router,
        "CreateDataQualityRuleset",
        &json!({ "Name": "stable", "Ruleset": "Rules = [ ColumnExists \"z\" ]" }),
    )
    .await;
    let recreated = ok(
        &router,
        "UpdateDataQualityRuleset",
        &json!({ "Name": "stable", "Description": "after the recreate" }),
    )
    .await;
    assert_ne!(
        member(&recreated, "Identifier").map(str::to_string),
        first,
        "case recreated under the same name: a new record must carry a new identifier, got \
         {recreated}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: an empty listing is an empty collection
// ---------------------------------------------------------------------------

/// Every name listing this service serves reports its collection member on an account that
/// holds nothing — as an empty list — and reports no continuation token, because no further
/// page follows.
#[tokio::test]
async fn an_empty_listing_reports_an_empty_collection_not_an_absent_member() {
    let router = emulator();

    // (the operation, the collection member it reports)
    let listings: &[(&str, &str)] = &[
        ("ListWorkflows", "Workflows"),
        ("ListCrawlers", "CrawlerNames"),
        ("ListJobs", "JobNames"),
        ("ListTriggers", "TriggerNames"),
        ("ListBlueprints", "Blueprints"),
        ("ListRegistries", "Registries"),
        ("ListCustomEntityTypes", "CustomEntityTypes"),
        ("ListDataQualityRulesets", "Rulesets"),
    ];

    for (operation, collection) in listings {
        // Asked with no member at all, and asked with each published page bound, so the
        // rule is not a property of one request shape.
        for request in [json!({}), json!({ "MaxResults": 1 })] {
            let answered = ok(&router, operation, &request).await;
            let reported = answered.get(*collection).unwrap_or_else(|| {
                panic!(
                    "case {operation} with {request}: expected the {collection} member, got \
                     {answered}"
                )
            });
            assert_eq!(
                reported,
                &json!([]),
                "case {operation} with {request}: an empty listing is an empty list, got \
                 {answered}"
            );
            assert!(
                answered.get("NextToken").is_none_or(Value::is_null),
                "case {operation} with {request}: no further page follows, so no token is \
                 reported, got {answered}"
            );
        }
    }
}

/// A page past the END of a populated listing reports the same empty collection: the member
/// is present, holds no name, and carries no token. The rows differ along where the cursor
/// sits relative to the stored names.
#[tokio::test]
async fn a_page_past_the_end_of_a_listing_reports_an_empty_collection() {
    let router = emulator();
    for name in ["alpha", "beta", "gamma"] {
        ok(&router, "CreateWorkflow", &json!({ "Name": name })).await;
    }

    for (case, cursor) in [
        ("a cursor at the last stored name", "gamma"),
        ("a cursor past every stored name", "zzz-beyond"),
        ("a cursor no listing ever issued", "\u{7f}"),
    ] {
        let answered = ok(&router, "ListWorkflows", &json!({ "NextToken": cursor })).await;
        assert_eq!(
            answered.get("Workflows"),
            Some(&json!([])),
            "case {case:?}: expected an empty Workflows list, got {answered}"
        );
        assert!(
            answered.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: expected no continuation token, got {answered}"
        );
    }

    // The listing itself is unchanged by those reads: the three names are still there, and
    // the first page still reports them.
    let served = ok(&router, "ListWorkflows", &json!({})).await;
    assert_eq!(
        served.get("Workflows"),
        Some(&json!(["alpha", "beta", "gamma"])),
        "case after the exhausted pages: the listing must still report every stored name, got \
         {served}"
    );
}
