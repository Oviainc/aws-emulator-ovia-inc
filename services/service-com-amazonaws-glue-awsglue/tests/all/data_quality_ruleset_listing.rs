// SPDX-License-Identifier: CC0-1.0

//! `ListDataQualityRulesets`: the data quality rulesets this account holds.
//!
//! RULE: the operation "returns a paginated list of rulesets", so it reports every ruleset
//! the store holds — the records `CreateDataQualityRuleset` wrote and
//! `GetDataQualityRuleset` reads back — one `MaxResults`-sized page at a time with a
//! `NextToken` that resumes the walk, narrowed to the rulesets every member of a `Filter`
//! admits and to those carrying every pair a `Tags` filter names, each entry carrying the
//! number of rules its stored DQDL declares.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many rulesets are stored, the
//! page size asked for (including the published bounds 1 and 1000 and a size past the
//! stored count), which criterion the filter names, which tags it names, and the form of
//! the DQDL each stored ruleset carries.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, ok};

/// Register one ruleset, with the members the row asks for.
async fn given_ruleset(router: &MultiServiceRouter, body: &Value) {
    ok(router, "CreateDataQualityRuleset", body).await;
}

/// The entries of one answer, in the order the service returned them. The member is
/// present even when it reports nothing.
fn listed(page: &Value) -> Vec<Value> {
    page.get("Rulesets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must report a Rulesets list, got {page}"))
        .clone()
}

/// The `Name` of every entry of one answer, in the order the service returned them.
fn listed_names(page: &Value) -> Vec<String> {
    listed(page)
        .iter()
        .map(|entry| {
            entry["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry carries a Name, got {entry}"))
                .to_string()
        })
        .collect()
}

/// Walk every page the service hands out for `base`, following its own `NextToken`, and
/// return the entries in the order they were reported.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    base: &Value,
    page_size: Option<i32>,
    limit: usize,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = base.clone();
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "ListDataQualityRulesets", &request).await;
        let entries = listed(&page);
        if let Some(size) = page_size {
            assert!(
                entries.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {page}"
            );
        }
        collected.extend(entries);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            collected.len() <= limit,
            "case {case:?}: the walk must terminate, got {collected:?} for {limit} rulesets"
        );
    }
    collected
}

/// Every stored ruleset is reported, in ruleset-name order, and a page never exceeds the
/// size the request asked for.
#[tokio::test]
async fn every_stored_ruleset_is_listed_and_paged() {
    // (case, how many rulesets are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no rulesets at all", 0, None),
        ("one ruleset, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize` range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("ruleset-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_ruleset(
                &router,
                &json!({
                    "Name": name,
                    "Ruleset": format!("Rules = [ ColumnExists \"{name}\" ]"),
                    "Description": format!("the {name} checks"),
                }),
            )
            .await;
        }

        let collected = walk(&router, case, &json!({}), *page_size, *stored).await;
        let names: Vec<String> = collected
            .iter()
            .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected every stored ruleset in name order, got {names:?}"
        );

        // A ruleset the listing reports is one `GetDataQualityRuleset` resolves, with the
        // same definition: one record, two reads.
        for entry in &collected {
            let name = entry["Name"].as_str().expect("the entry carries a Name");
            let read = ok(
                &router,
                "GetDataQualityRuleset",
                &json!({ "Name": name }),
            )
            .await;
            for member in ["Description", "CreatedOn", "LastModifiedOn"] {
                assert_eq!(
                    read.get(member),
                    entry.get(member),
                    "case {case:?}: the listed {name} must report the stored {member}, got \
                     {read} for {entry}"
                );
            }
            // `DataQualityRulesetListDetails` models no `Ruleset` member, so a listing
            // reports how many rules a ruleset holds without serving the rules themselves.
            assert!(
                entry.get("Ruleset").is_none(),
                "case {case:?}: a listed ruleset carries no Ruleset text, got {entry}"
            );
        }
    }
}

/// `RuleCount` is "the number of rules in the ruleset": the number of entries the stored
/// DQDL's own rule list declares, whatever each rule's text is. The rows differ along the
/// form of that list — how many rules, how they are laid out, what punctuation a rule's own
/// text carries, and whether a rule list is declared at all.
#[tokio::test]
async fn a_listed_ruleset_reports_the_number_of_rules_its_dqdl_declares() {
    let router = emulator();

    // (case, the stored DQDL, the number of rules it declares)
    let rows: &[(&str, &str, i64)] = &[
        ("one rule", "Rules = [ ColumnExists \"a\" ]", 1),
        (
            "two rules on one line",
            "Rules = [ IsComplete \"a\", RowCount > 0 ]",
            2,
        ),
        (
            "three rules, one per line",
            "Rules = [\n    ColumnExists \"a\",\n    Completeness \"a\" > 0.95,\n    Uniqueness \"a\" > 0.9\n]",
            3,
        ),
        (
            "no space around the assignment",
            "Rules=[ColumnExists \"a\",ColumnExists \"b\"]",
            2,
        ),
        (
            "a rule carrying its own bracketed list",
            "Rules = [ ColumnValues \"grade\" in [ \"a\", \"b\", \"c\" ], RowCount > 0 ]",
            2,
        ),
        (
            "a column name carrying a comma",
            "Rules = [ ColumnExists \"last, first\" ]",
            1,
        ),
        (
            "a rule carrying a parenthesised expression",
            "Rules = [ (ColumnExists \"a\") and (ColumnExists \"b\") ]",
            1,
        ),
        (
            "a trailing separator declares no further rule",
            "Rules = [ ColumnExists \"a\", ]",
            1,
        ),
        (
            "a commented-out rule is not declared",
            "Rules = [\n    ColumnExists \"a\"\n    # , ColumnExists \"b\"\n]",
            1,
        ),
        ("an empty rule list is refused rather than stored", "Rules = []", 0),
        (
            "no rule list at all is refused rather than stored",
            "Analyzers = [ Completeness \"a\" ]",
            0,
        ),
        (
            "a ruleset whose analyzers follow its rules",
            "Rules = [ ColumnExists \"a\", RowCount > 0 ]\nAnalyzers = [ Completeness \"a\", DistinctValuesCount \"a\" ]",
            2,
        ),
        (
            "a column named after the rule-list keyword",
            "Rules = [ ColumnExists \"Rules\" ]",
            1,
        ),
    ];

    for (index, (case, ruleset, expected)) in rows.iter().enumerate() {
        let name = format!("counted-{index:02}");
        // DQDL declaring NO rule is not a ruleset the store can hold: the write refuses it
        // (see [`crate::data_quality_ruleset_parsing`]), so there is nothing to list.
        if *expected == 0 {
            let (status, body) = call(
                &router,
                "CreateDataQualityRuleset",
                &json!({ "Name": name, "Ruleset": ruleset }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the create to be refused, got {status} with {body}"
            );
            let page = ok(
                &router,
                "ListDataQualityRulesets",
                &json!({ "Filter": { "Name": name } }),
            )
            .await;
            assert!(
                listed(&page).is_empty(),
                "case {case:?}: a refused create must leave nothing to list, got {page}"
            );
            continue;
        }
        given_ruleset(
            &router,
            &json!({ "Name": name, "Ruleset": ruleset }),
        )
        .await;

        let page = ok(
            &router,
            "ListDataQualityRulesets",
            &json!({ "Filter": { "Name": name } }),
        )
        .await;
        let entries = listed(&page);
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected exactly the ruleset named {name}, got {page}"
        );
        assert_eq!(
            entries[0].get("RuleCount").and_then(Value::as_i64),
            Some(*expected),
            "case {case:?}: expected RuleCount {expected} for {ruleset:?}, got {}",
            entries[0]
        );
    }

    // The count follows the DQDL the store holds NOW: an update that replaces the rules
    // changes it, so the member is computed from the record rather than fixed at create.
    let updated = "counted-00";
    ok(
        &router,
        "UpdateDataQualityRuleset",
        &json!({
            "Name": updated,
            "Ruleset": "Rules = [ ColumnExists \"a\", ColumnExists \"b\", RowCount > 0 ]",
        }),
    )
    .await;
    let page = ok(
        &router,
        "ListDataQualityRulesets",
        &json!({ "Filter": { "Name": updated } }),
    )
    .await;
    assert_eq!(
        listed(&page)
            .first()
            .and_then(|entry| entry.get("RuleCount"))
            .and_then(Value::as_i64),
        Some(3),
        "case after an update: expected the rules the stored DQDL now declares, got {page}"
    );
}

/// The longest `Ruleset` the model accepts: "len 1..65536" on `DataQualityRulesetString`
/// (`smithy_query operation=CreateDataQualityRuleset`).
const MAX_RULESET_LENGTH: usize = 65_536;

/// DQDL no rule list can be read out of — malformed, truncated or nested past what a rule is
/// written with — declares no rules, so a write carrying it is REFUSED and nothing is listed
/// for it (the refusal's own wording is pinned by
/// [`crate::data_quality_ruleset_parsing`]). Text that does declare rules is stored and
/// listed with that many, so the listing answers for every ruleset the store can hold and
/// text a caller chose can neither fail nor hang it. A ruleset past the modelled length
/// bound never reaches the store at all.
#[tokio::test]
async fn dqdl_no_rule_list_can_be_read_out_of_is_refused_rather_than_listed() {
    let router = emulator();

    // A rule list nested far past anything a rule's own operands need.
    let overly_nested = format!("Rules = [ {} ]", "[".repeat(64));
    // One rule whose operands nest a few levels, which is inside what the walk follows.
    let nested_rule = "Rules = [ ColumnValues \"grade\" in [ \"a\", \"b\" ], RowCount > 0 ]";
    // A rule list that fills the modelled length bound exactly, so the count is taken over
    // the whole text and not only its head.
    let rule = "ColumnExists \"c\",";
    let head = "Rules = [";
    let filled = (MAX_RULESET_LENGTH - head.len() - 1) / rule.len();
    let at_the_bound = format!(
        "{head}{}{}]",
        rule.repeat(filled),
        " ".repeat(MAX_RULESET_LENGTH - head.len() - 1 - filled * rule.len())
    );
    assert_eq!(
        at_the_bound.len(),
        MAX_RULESET_LENGTH,
        "the padded ruleset must sit exactly on the modelled bound, got {}",
        at_the_bound.len()
    );
    let filled = i64::try_from(filled).expect("the rule count fits an i64");

    // (case, the stored DQDL, the number of rules it declares)
    let rows: &[(&str, &str, i64)] = &[
        ("an unterminated rule list", "Rules = [ ColumnExists \"a\"", 0),
        ("truncated after the keyword", "Rules", 0),
        ("truncated after the assignment", "Rules =", 0),
        (
            "an unterminated string swallowing the closing bracket",
            "Rules = [ ColumnExists \"a ]",
            0,
        ),
        (
            "a comment swallowing the closing bracket",
            "Rules = [ ColumnExists \"a\" # ]",
            0,
        ),
        ("a single blank", " ", 0),
        ("punctuation alone", "][,,,", 0),
        ("the keyword without a list", "Rules = ColumnExists \"a\"", 0),
        (
            "the keyword as the tail of a longer word",
            "MyRules = [ ColumnExists \"a\" ]",
            0,
        ),
        (
            "the keyword's own prefix running into it",
            "RuRules = [ ColumnExists \"a\" ]",
            0,
        ),
        (
            "a word of its own after that prefix",
            "Ru Rules = [ ColumnExists \"a\" ]",
            1,
        ),
        ("rules nested past what a rule uses", &overly_nested, 0),
        ("a rule whose operands nest", nested_rule, 2),
        (
            "a rule list at the modelled length bound",
            &at_the_bound,
            filled,
        ),
    ];

    for (index, (case, ruleset, expected)) in rows.iter().enumerate() {
        let name = format!("malformed-{index:02}");
        // Text declaring no rule is refused by the write, so nothing is listed for it; text
        // declaring rules is stored and listed with that many.
        if *expected == 0 {
            let (status, body) = call(
                &router,
                "CreateDataQualityRuleset",
                &json!({ "Name": name, "Ruleset": ruleset }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the create to be refused, got {status} with {body}"
            );
            let page = ok(
                &router,
                "ListDataQualityRulesets",
                &json!({ "Filter": { "Name": name } }),
            )
            .await;
            assert!(
                listed(&page).is_empty(),
                "case {case:?}: a refused create must leave nothing to list, got {page}"
            );
            continue;
        }
        given_ruleset(&router, &json!({ "Name": name, "Ruleset": ruleset })).await;

        let page = ok(
            &router,
            "ListDataQualityRulesets",
            &json!({ "Filter": { "Name": name } }),
        )
        .await;
        let entries = listed(&page);
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected exactly the ruleset named {name}, got {page}"
        );
        assert_eq!(
            entries[0].get("RuleCount").and_then(Value::as_i64),
            Some(*expected),
            "case {case:?}: expected RuleCount {expected}, got {}",
            entries[0]
        );
    }

    // One past the modelled length bound is refused by the constraint rather than stored,
    // so the longest text the counter can ever read is the row above.
    let (status, body) = call(
        &router,
        "CreateDataQualityRuleset",
        &json!({
            "Name": "oversized",
            "Ruleset": "x".repeat(MAX_RULESET_LENGTH + 1),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one character past the length bound: expected a refusal, got {status} {body}"
    );
}

/// A `Filter` is a conjunction over the stored record: each member it carries narrows the
/// listing and a member it leaves out narrows nothing. The rows differ along which member
/// the criteria name and whether the stored ruleset satisfies it.
#[tokio::test]
async fn a_filter_narrows_the_listing_to_the_rulesets_it_admits() {
    let router = emulator();
    given_ruleset(
        &router,
        &json!({
            "Name": "orders",
            "Ruleset": "Rules = [ ColumnExists \"order_id\" ]",
            "Description": "checks the orders table",
            "TargetTable": { "DatabaseName": "sales", "TableName": "orders" },
        }),
    )
    .await;
    given_ruleset(
        &router,
        &json!({
            "Name": "customers",
            "Ruleset": "Rules = [ IsComplete \"customer_id\", RowCount > 0 ]",
            "Description": "checks the customers table",
            "TargetTable": {
                "DatabaseName": "sales",
                "TableName": "customers",
                "CatalogId": crate::wire::ACCOUNT_ID,
            },
        }),
    )
    .await;
    given_ruleset(
        &router,
        &json!({
            "Name": "untargeted",
            "Ruleset": "Rules = [ RowCount > 0 ]",
        }),
    )
    .await;

    // (case, the Filter, the names it must report, in name order)
    let rows: &[(&str, Value, &[&str])] = &[
        (
            "no criterion narrows nothing",
            json!({}),
            &["customers", "orders", "untargeted"],
        ),
        ("a stored name", json!({ "Name": "orders" }), &["orders"]),
        ("a name nothing carries", json!({ "Name": "no-such" }), &[]),
        (
            "a prefix of a stored name is a different name",
            json!({ "Name": "order" }),
            &[],
        ),
        (
            "a stored description",
            json!({ "Description": "checks the customers table" }),
            &["customers"],
        ),
        (
            "a description nothing carries",
            json!({ "Description": "checks nothing" }),
            &[],
        ),
        (
            "a target table",
            json!({ "TargetTable": { "DatabaseName": "sales", "TableName": "orders" } }),
            &["orders"],
        ),
        (
            "a target table in another database",
            json!({ "TargetTable": { "DatabaseName": "other", "TableName": "orders" } }),
            &[],
        ),
        (
            "a target table naming its catalog",
            json!({
                "TargetTable": {
                    "DatabaseName": "sales",
                    "TableName": "customers",
                    "CatalogId": crate::wire::ACCOUNT_ID,
                },
            }),
            &["customers"],
        ),
        (
            "a target table naming another catalog",
            json!({
                "TargetTable": {
                    "DatabaseName": "sales",
                    "TableName": "customers",
                    "CatalogId": "999999999999",
                },
            }),
            &[],
        ),
        (
            "a target table no stored ruleset targets at all",
            json!({ "TargetTable": { "DatabaseName": "sales", "TableName": "untargeted" } }),
            &[],
        ),
        (
            "a name and a description that agree",
            json!({ "Name": "orders", "Description": "checks the orders table" }),
            &["orders"],
        ),
        (
            "a name and a description that disagree",
            json!({ "Name": "orders", "Description": "checks the customers table" }),
            &[],
        ),
    ];

    for (case, filter, expected) in rows {
        let request = json!({ "Filter": filter });
        let page = ok(&router, "ListDataQualityRulesets", &request).await;
        let want: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            listed_names(&page),
            want,
            "case {case:?}: expected the rulesets satisfying {filter}, got {page}"
        );

        // A filter narrows a PAGED walk too, so the token resumes within the filtered set
        // rather than over the whole store.
        let paged = walk(&router, case, &request, Some(1), 3).await;
        let paged_names: Vec<String> = paged
            .iter()
            .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            paged_names, want,
            "case {case:?}: a one-per-page walk must report the same rulesets, got {paged:?}"
        );
    }
}

/// An instant criterion bounds the window the listing reports, on each of the two pairs the
/// criteria declare.
#[tokio::test]
async fn an_instant_criterion_bounds_the_window_the_listing_reports() {
    let router = emulator();
    given_ruleset(
        &router,
        &json!({ "Name": "dated", "Ruleset": "Rules = [ RowCount > 0 ]" }),
    )
    .await;

    let page = ok(&router, "ListDataQualityRulesets", &json!({})).await;
    let entry = listed(&page)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the stored ruleset must be listed, got {page}"));
    let created_on = entry["CreatedOn"]
        .as_f64()
        .unwrap_or_else(|| panic!("CreatedOn must be a timestamp, got {entry}"));
    // One second each side of the ruleset's own instant, so each row sits firmly inside or
    // outside the window the criterion describes rather than on its edge, where the
    // reported instant is only a rendering of the stored one.
    let (before, after) = (created_on - 1.0, created_on + 1.0);

    // (case, the Filter, whether the ruleset is inside the window it describes)
    let rows: &[(&str, Value, bool)] = &[
        (
            "created before a later instant",
            json!({ "CreatedBefore": after }),
            true,
        ),
        (
            "created before an earlier instant",
            json!({ "CreatedBefore": before }),
            false,
        ),
        (
            "created after an earlier instant",
            json!({ "CreatedAfter": before }),
            true,
        ),
        (
            "created after a later instant",
            json!({ "CreatedAfter": after }),
            false,
        ),
        (
            "last modified before a later instant",
            json!({ "LastModifiedBefore": after }),
            true,
        ),
        (
            "last modified before an earlier instant",
            json!({ "LastModifiedBefore": before }),
            false,
        ),
        (
            "last modified after an earlier instant",
            json!({ "LastModifiedAfter": before }),
            true,
        ),
        (
            "last modified after a later instant",
            json!({ "LastModifiedAfter": after }),
            false,
        ),
        (
            "a window around the instant",
            json!({ "CreatedAfter": before, "CreatedBefore": after }),
            true,
        ),
        (
            "a window that closes before it",
            json!({ "CreatedAfter": before - 1.0, "CreatedBefore": before }),
            false,
        ),
        (
            "a window that opens after it",
            json!({ "CreatedAfter": after, "CreatedBefore": after + 1.0 }),
            false,
        ),
    ];

    for (case, filter, inside) in rows {
        let page = ok(
            &router,
            "ListDataQualityRulesets",
            &json!({ "Filter": filter }),
        )
        .await;
        let expected: Vec<String> = if *inside {
            vec!["dated".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            listed_names(&page),
            expected,
            "case {case:?}: expected the rulesets inside {filter}, got {page}"
        );
    }
}

/// A `Tags` filter narrows the listing to the rulesets carrying EVERY pair it names.
#[tokio::test]
async fn a_tag_filter_narrows_the_listing_to_the_rulesets_carrying_every_pair() {
    let router = emulator();
    for (name, tags) in [
        ("tagged-both", json!({ "team": "data", "env": "prod" })),
        ("tagged-team", json!({ "team": "data" })),
        ("tagged-other", json!({ "team": "ops" })),
        ("untagged", Value::Null),
    ] {
        let mut body = json!({
            "Name": name,
            "Ruleset": format!("Rules = [ ColumnExists \"{name}\" ]"),
        });
        if !tags.is_null() {
            body["Tags"] = tags;
        }
        given_ruleset(&router, &body).await;
    }

    // (case, the Tags filter, the names it must report, in name order)
    let rows: &[(&str, Value, &[&str])] = &[
        (
            "no filter: every ruleset",
            Value::Null,
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "an empty filter narrows nothing",
            json!({}),
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "one pair",
            json!({ "team": "data" }),
            &["tagged-both", "tagged-team"],
        ),
        (
            "a pair that must BOTH match",
            json!({ "team": "data", "env": "prod" }),
            &["tagged-both"],
        ),
        ("a value nothing carries", json!({ "team": "finance" }), &[]),
        ("a key nothing carries", json!({ "owner": "data" }), &[]),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({});
        if !filter.is_null() {
            request["Tags"] = filter.clone();
        }
        let page = ok(&router, "ListDataQualityRulesets", &request).await;
        assert_eq!(
            listed_names(&page),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the rulesets carrying every pair of {filter}, got {page}"
        );
    }

    // `DataQualityRulesetListDetails` models no `Tags` member, so a listed ruleset reports
    // none even where the filter selected it by one.
    let page = ok(
        &router,
        "ListDataQualityRulesets",
        &json!({ "Tags": { "team": "data" } }),
    )
    .await;
    for entry in listed(&page) {
        assert!(
            entry.get("Tags").is_none(),
            "case a listed ruleset's members: expected no Tags member, got {entry}"
        );
    }
}

/// The listing reports what the store holds NOW, and reports it without changing anything
/// a later caller sees.
#[tokio::test]
async fn the_listing_is_a_report_of_the_store_as_it_stands() {
    let router = emulator();
    for name in ["first", "second"] {
        given_ruleset(
            &router,
            &json!({
                "Name": name,
                "Ruleset": format!("Rules = [ ColumnExists \"{name}\" ]"),
            }),
        )
        .await;
    }

    let before = ok(&router, "ListDataQualityRulesets", &json!({})).await;
    assert_eq!(
        listed_names(&before),
        vec!["first".to_string(), "second".to_string()],
        "case before the third create: expected every stored ruleset, got {before}"
    );

    // Two listings in a row agree, so reporting changed nothing a later caller sees.
    let again = ok(&router, "ListDataQualityRulesets", &json!({})).await;
    assert_eq!(
        listed(&again),
        listed(&before),
        "case listed twice: a listing must change nothing, got {again} then {before}"
    );

    // A ruleset created after the first listing is reported by the next one.
    given_ruleset(
        &router,
        &json!({ "Name": "third", "Ruleset": "Rules = [ RowCount > 0 ]" }),
    )
    .await;
    let after = ok(&router, "ListDataQualityRulesets", &json!({})).await;
    assert_eq!(
        listed_names(&after),
        vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string()
        ],
        "case after the third create: expected the ruleset the store now holds, got {after}"
    );

    // An update is reported by the next listing too: `LastModifiedOn` moves while
    // `CreatedOn` stays where the create put it.
    let listed_first = listed(&after)
        .into_iter()
        .find(|entry| entry["Name"] == json!("first"))
        .unwrap_or_else(|| panic!("the stored ruleset must be listed, got {after}"));
    ok(
        &router,
        "UpdateDataQualityRuleset",
        &json!({ "Name": "first", "Description": "updated after the listing" }),
    )
    .await;
    let updated = ok(&router, "ListDataQualityRulesets", &json!({})).await;
    let entry = listed(&updated)
        .into_iter()
        .find(|entry| entry["Name"] == json!("first"))
        .unwrap_or_else(|| panic!("the updated ruleset must be listed, got {updated}"));
    assert_eq!(
        entry.get("Description").and_then(Value::as_str),
        Some("updated after the listing"),
        "case after an update: expected the stored description, got {entry}"
    );
    assert_eq!(
        entry.get("CreatedOn"),
        listed_first.get("CreatedOn"),
        "case after an update: CreatedOn must not move, got {entry} after {listed_first}"
    );
    assert!(
        entry["LastModifiedOn"].as_f64() >= listed_first["LastModifiedOn"].as_f64(),
        "case after an update: LastModifiedOn must not go backwards, got {entry} after \
         {listed_first}"
    );
}

/// `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused by
/// the modelled constraint rather than served as some other page size.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    given_ruleset(
        &router,
        &json!({ "Name": "bounded", "Ruleset": "Rules = [ RowCount > 0 ]" }),
    )
    .await;

    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1_001)] {
        let (status, body) = call(
            &router,
            "ListDataQualityRulesets",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }
}
