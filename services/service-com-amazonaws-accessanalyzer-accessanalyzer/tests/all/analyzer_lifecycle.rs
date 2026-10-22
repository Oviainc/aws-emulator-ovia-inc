// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the analyzer slice, driven over the wire.
//!
//! Every case here goes through the real router: an `http::Request` is dispatched to
//! the mounted service, so the restJson1 routing, the boundary constraint validation,
//! the handler and the response serialization all run exactly as they do in the
//! emulator binary. Nothing reaches into `crate::state` directly — a create is only
//! believed when a later `Get`/`List` call reports it back.
//!
//! The tables span the classes the rules generalise over (analyzer type, tag and
//! configuration shape, name form, page size, filter criterion), and the recorded
//! `demo`/`ACCOUNT`/`resourceType eq AWS::S3::Bucket` case from the issue is one row.

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::state::{
    MAX_ANALYZERS, MAX_ARCHIVE_RULES_PER_ANALYZER, MAX_PAGE_SIZE,
};

// The wire harness is shared by every suite in this target; see `tests/all/harness.rs`.
use crate::harness::{
    expected_arn, is_timestamp, listed_names, listed_rule_names, Emulator, Reply,
};

/// Every `Type` the model's `Type` enum accepts, from
/// `smithy_query(query="operation", operation="CreateAnalyzer")`'s `type` member.
const ANALYZER_TYPES: &[&str] = &[
    "ACCOUNT",
    "ACCOUNT_INTERNAL_ACCESS",
    "ACCOUNT_UNUSED_ACCESS",
    "ORGANIZATION",
    "ORGANIZATION_INTERNAL_ACCESS",
    "ORGANIZATION_UNUSED_ACCESS",
];

/// One row of the create/read-back table.
#[derive(Debug)]
struct AnalyzerCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    name: String,
    analyzer_type: &'static str,
    /// The `tags` the create carries, `None` when it carries none.
    tags: Option<Value>,
    /// The `configuration` the create carries, `None` when it carries none.
    configuration: Option<Value>,
}

/// A create followed by a read reports back exactly what was created — over the whole
/// class of analyzer shapes, not just the recorded one.
#[tokio::test]
async fn an_analyzer_reads_back_as_it_was_created() {
    // The rows differ along the members the rule generalises over: every analyzer
    // type, a name at each end of the `len 1..255` bound and in each form the
    // `^[A-Za-z_][A-Za-z0-9_.-]*$` pattern allows, tags absent and present, and each
    // `AnalyzerConfiguration` union variant as well as none.
    let cases = vec![
        AnalyzerCase {
            case: "the recorded case",
            name: "demo".to_string(),
            analyzer_type: "ACCOUNT",
            tags: Some(json!({"env": "test"})),
            configuration: None,
        },
        AnalyzerCase {
            case: "one-character name, no tags, no configuration",
            name: "a".to_string(),
            analyzer_type: "ORGANIZATION",
            tags: None,
            configuration: None,
        },
        AnalyzerCase {
            case: "leading underscore, several tags",
            name: "_leading_underscore".to_string(),
            analyzer_type: "ACCOUNT_INTERNAL_ACCESS",
            tags: Some(json!({"owner": "team", "cost-center": "42", "note": "a b"})),
            configuration: Some(json!({"internalAccess": {"analysisRule": {"inclusions": [
                {"resourceTypes": ["AWS::S3::Bucket"]}
            ]}}})),
        },
        AnalyzerCase {
            case: "dots and dashes, unused-access configuration",
            name: "dots.and-dashes_9".to_string(),
            analyzer_type: "ACCOUNT_UNUSED_ACCESS",
            tags: None,
            configuration: Some(json!({"unusedAccess": {"unusedAccessAge": 90}})),
        },
        AnalyzerCase {
            case: "name at the 255-character maximum",
            name: format!("a{}", "n".repeat(254)),
            analyzer_type: "ORGANIZATION_UNUSED_ACCESS",
            tags: Some(json!({"k": "v"})),
            configuration: Some(json!({"unusedAccess": {"unusedAccessAge": 1}})),
        },
    ];

    let emulator = Emulator::new();
    let mut created: Vec<String> = Vec::new();

    for AnalyzerCase {
        case,
        name,
        analyzer_type,
        tags,
        configuration,
    } in &cases
    {
        let mut request = json!({"analyzerName": name, "type": analyzer_type});
        if let Some(tags) = tags {
            request["tags"] = tags.clone();
        }
        if let Some(configuration) = configuration {
            request["configuration"] = configuration.clone();
        }

        let create = emulator.create_analyzer(request).await;
        assert_eq!(
            create.status, 200,
            "case {case:?}: CreateAnalyzer must succeed, got {create:?}"
        );
        assert_eq!(
            create.member("arn").and_then(Value::as_str),
            Some(expected_arn(name).as_str()),
            "case {case:?}: create must return the stored ARN, got {:?}",
            create.member("arn")
        );

        let get = emulator.get_analyzer(name).await;
        assert_eq!(
            get.status, 200,
            "case {case:?}: GetAnalyzer must succeed, got {get:?}"
        );
        let analyzer = get
            .member("analyzer")
            .unwrap_or_else(|| panic!("case {case:?}: GetAnalyzer must carry `analyzer`: {get:?}"));

        assert_eq!(
            analyzer.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: name must round-trip, got {:?}",
            analyzer.get("name")
        );
        assert_eq!(
            analyzer.get("type").and_then(Value::as_str),
            Some(*analyzer_type),
            "case {case:?}: type must round-trip, got {:?}",
            analyzer.get("type")
        );
        assert_eq!(
            analyzer.get("arn").and_then(Value::as_str),
            Some(expected_arn(name).as_str()),
            "case {case:?}: the summary must carry the ARN create returned, got {:?}",
            analyzer.get("arn")
        );
        assert_eq!(
            analyzer.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: a created analyzer is ACTIVE, got {:?}",
            analyzer.get("status")
        );
        assert!(
            is_timestamp(analyzer.get("createdAt")),
            "case {case:?}: createdAt must be the instant the analyzer was created, got {:?}",
            analyzer.get("createdAt")
        );
        assert_eq!(
            analyzer.get("tags"),
            tags.as_ref(),
            "case {case:?}: tags must be exactly what create supplied, got {:?}",
            analyzer.get("tags")
        );
        assert_eq!(
            analyzer.get("configuration"),
            configuration.as_ref(),
            "case {case:?}: configuration must be exactly what create supplied, got {:?}",
            analyzer.get("configuration")
        );
        assert!(
            analyzer.get("statusReason").is_none_or(Value::is_null),
            "case {case:?}: an ACTIVE analyzer has no statusReason, got {:?}",
            analyzer.get("statusReason")
        );

        // A read never changes what a later caller sees: the second read is identical.
        let again = emulator.get_analyzer(name).await;
        assert_eq!(
            again.member("analyzer"),
            Some(analyzer),
            "case {case:?}: re-reading must report the identical summary, got {again:?}"
        );

        created.push(name.clone());
    }

    // Every analyzer created above is stored, and the listing reports all of them.
    created.sort();
    let list = emulator.list_analyzers("").await;
    assert_eq!(
        listed_names(&list),
        created,
        "ListAnalyzers must report every stored analyzer, got {list:?}"
    );
}

/// The boundary refuses a request outside the modelled constraints instead of storing
/// it: each published bound, and one value past it.
#[tokio::test]
async fn an_analyzer_outside_the_modelled_constraints_is_refused() {
    // (case, analyzerName, type, expected status). The accepted rows sit ON the
    // published bounds of `AnalyzerName` (len 1..255, pattern
    // `^[A-Za-z_][A-Za-z0-9_.-]*$`) and in the `Type` enum; the refused rows sit one
    // step past each of them.
    let at_max = format!("b{}", "x".repeat(254));
    let past_max = format!("c{}", "x".repeat(255));
    let cases: Vec<(&str, String, &str, u16)> = vec![
        ("minimum length", "z".to_string(), "ACCOUNT", 200),
        ("maximum length", at_max, "ACCOUNT", 200),
        ("one past the maximum length", past_max, "ACCOUNT", 400),
        ("empty name is below the minimum", String::new(), "ACCOUNT", 400),
        (
            "a name the pattern rejects (leading digit)",
            "1analyzer".to_string(),
            "ACCOUNT",
            400,
        ),
        (
            "a name the pattern rejects (space)",
            "has space".to_string(),
            "ACCOUNT",
            400,
        ),
        (
            "a type outside the enum",
            "typed".to_string(),
            "ACCOUNT_EXTERNAL_ACCESS",
            400,
        ),
    ];

    for (case, name, analyzer_type, expected) in &cases {
        let emulator = Emulator::new();
        let reply = emulator
            .create_analyzer(json!({"analyzerName": name, "type": analyzer_type}))
            .await;
        assert_eq!(
            reply.status, *expected,
            "case {case:?}: expected HTTP {expected}, got {reply:?}"
        );

        if *expected == 400 {
            // A refused create stores nothing, so no analyzer is listed at all.
            // (Checked through the listing, not `GetAnalyzer`: an empty name is not a
            // path this service routes to `GetAnalyzer` in the first place.)
            let list = emulator.list_analyzers("").await;
            assert_eq!(
                listed_names(&list),
                Vec::<String>::new(),
                "case {case:?}: a refused create must store nothing, got {list:?}"
            );
        }
    }

    // Every enum value the model publishes is accepted, each in its own emulator.
    for analyzer_type in ANALYZER_TYPES {
        let emulator = Emulator::new();
        let reply = emulator
            .create_analyzer(json!({"analyzerName": "typed", "type": analyzer_type}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {analyzer_type:?}: a modelled analyzer type must be accepted, got {reply:?}"
        );
    }
}

/// Re-creating a stored name conflicts; retrying the SAME create with its client
/// token answers with the first result instead.
#[tokio::test]
async fn recreating_a_name_conflicts_unless_the_client_token_repeats() {
    let emulator = Emulator::new();

    let first = emulator
        .create_analyzer(json!({"analyzerName": "demo", "type": "ACCOUNT"}))
        .await;
    assert_eq!(first.status, 200, "the first create must succeed: {first:?}");

    // Same name, no client token: a second, distinct create of an existing analyzer.
    for case in ["ACCOUNT", "ORGANIZATION"] {
        let conflict = emulator
            .create_analyzer(json!({"analyzerName": "demo", "type": case}))
            .await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: re-creating a stored name must be a 409, got {conflict:?}"
        );
        assert_eq!(
            conflict.code(),
            "ConflictException",
            "case {case:?}: the 409 must be a ConflictException, got {conflict:?}"
        );
    }

    // Same name AND same client token: the same request retried.
    let request = json!({"analyzerName": "tokened", "type": "ACCOUNT", "clientToken": "token-1"});
    let created = emulator.create_analyzer(request.clone()).await;
    assert_eq!(created.status, 200, "the tokened create must succeed: {created:?}");
    for attempt in 1..=2 {
        let retry = emulator.create_analyzer(request.clone()).await;
        assert_eq!(
            retry.status, 200,
            "case retry {attempt}: a repeated clientToken must not conflict, got {retry:?}"
        );
        assert_eq!(
            retry.member("arn"),
            created.member("arn"),
            "case retry {attempt}: a repeated clientToken must return the first ARN, got {:?}",
            retry.member("arn")
        );
    }

    // The same token with a DIFFERENT name is a different request, and creates.
    let other = emulator
        .create_analyzer(json!({"analyzerName": "other", "type": "ACCOUNT", "clientToken": "token-1"}))
        .await;
    assert_eq!(
        other.status, 200,
        "a token reused for another name is a new create, got {other:?}"
    );
    assert_eq!(
        other.member("arn").and_then(Value::as_str),
        Some(expected_arn("other").as_str()),
        "the new create must carry its own ARN, got {:?}",
        other.member("arn")
    );

    // ...and it does not displace the record of the request that first used the token:
    // retrying THAT request still resolves to its own first result.
    let retried = emulator.create_analyzer(request).await;
    assert_eq!(
        retried.member("arn"),
        created.member("arn"),
        "reusing a token elsewhere must not disturb the first request's result, got {retried:?}"
    );
}

/// `ListAnalyzers` reports stored state: filtered by `type` when asked, paged by
/// `maxResults`, resumable by `nextToken`, and refusing paging arguments it cannot honour.
#[tokio::test]
async fn listing_analyzers_filters_and_pages_over_stored_state() {
    let emulator = Emulator::new();
    // (name, type) — several analyzers per type, created out of name order so the
    // listing's order cannot come from insertion order.
    let fixtures = [
        ("gamma", "ACCOUNT"),
        ("alpha", "ACCOUNT"),
        ("delta", "ORGANIZATION"),
        ("beta", "ACCOUNT_UNUSED_ACCESS"),
        ("epsilon", "ORGANIZATION"),
    ];
    for (name, analyzer_type) in &fixtures {
        let reply = emulator
            .create_analyzer(json!({"analyzerName": name, "type": analyzer_type}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: the fixture create must succeed, got {reply:?}"
        );
    }
    let all_names = vec![
        "alpha".to_string(),
        "beta".to_string(),
        "delta".to_string(),
        "epsilon".to_string(),
        "gamma".to_string(),
    ];

    // Filtering by type reports exactly the analyzers of that type — including the
    // empty answer for a type nothing was created with.
    let filters: Vec<(&str, Vec<&str>)> = vec![
        ("ACCOUNT", vec!["alpha", "gamma"]),
        ("ACCOUNT_UNUSED_ACCESS", vec!["beta"]),
        ("ORGANIZATION", vec!["delta", "epsilon"]),
        ("ORGANIZATION_INTERNAL_ACCESS", vec![]),
    ];
    for (analyzer_type, expected) in &filters {
        let reply = emulator
            .list_analyzers(&format!("?type={analyzer_type}"))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {analyzer_type:?}: the filtered listing must succeed, got {reply:?}"
        );
        assert_eq!(
            listed_names(&reply),
            *expected,
            "case {analyzer_type:?}: the listing must report exactly the analyzers of that type, got {:?}",
            listed_names(&reply)
        );
    }

    // Paging: whatever the page size, following `nextToken` to exhaustion reports
    // every analyzer exactly once, in the same order, and no page exceeds the size.
    for page_size in [1_usize, 2, 3, 5, 9] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let reply = emulator.list_analyzers(&query).await;
            assert_eq!(
                reply.status, 200,
                "case page size {page_size}: a page must succeed, got {reply:?}"
            );
            let names = listed_names(&reply);
            assert!(
                names.len() <= page_size,
                "case page size {page_size}: a page must hold at most {page_size} items, got {names:?}"
            );
            seen.extend(names);
            token = reply
                .member("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= all_names.len() + 1,
                "case page size {page_size}: paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all_names,
            "case page size {page_size}: paging must report every analyzer once, in order, got {seen:?}"
        );
    }

    // Paging arguments the service cannot honour are refused, not silently ignored.
    // The forged tokens cover both forms: one that is not even the token's encoding,
    // and one that IS valid hex (`68656c6c6f` is "hello") but carries no marker this
    // service issued, so a hex-shaped guess cannot be used as a cursor.
    let refusals = [
        ("maxResults below one", "?maxResults=0"),
        ("negative maxResults", "?maxResults=-3"),
        ("a token this service never issued", "?nextToken=not-a-token"),
        ("a forged token that is valid hex", "?nextToken=68656c6c6f"),
    ];
    for (case, query) in &refusals {
        let reply = emulator.list_analyzers(query).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: the listing must refuse with a 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: the refusal must be a ValidationException, got {reply:?}"
        );
    }
}

/// An archive rule round-trips through state, under the analyzer it was created on.
#[tokio::test]
async fn an_archive_rule_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    let create = emulator
        .create_analyzer(json!({"analyzerName": "demo", "type": "ACCOUNT"}))
        .await;
    assert_eq!(create.status, 200, "the analyzer create must succeed: {create:?}");

    // (case, ruleName, filter). The rows differ along the `Criterion` members the
    // model publishes (`eq`, `neq`, `contains`, `exists`), the number of criteria in
    // the filter, and the name forms `^[A-Za-z][A-Za-z0-9_.-]*$` allows.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "the recorded case",
            "rule1",
            json!({"resourceType": {"eq": ["AWS::S3::Bucket"]}}),
        ),
        (
            "two criteria, contains and eq",
            "Rule.two-2",
            json!({"principal.AWS": {"contains": ["123456789012"]}, "isPublic": {"eq": ["true"]}}),
        ),
        (
            "neq with several values",
            "r_3",
            json!({"resource": {"neq": ["arn:aws:s3:::one", "arn:aws:s3:::two"]}}),
        ),
        ("exists", "R4", json!({"error": {"exists": true}})),
    ];

    let mut rule_names: Vec<String> = Vec::new();
    for (case, rule_name, filter) in &cases {
        let created = emulator
            .create_archive_rule("demo", json!({"ruleName": rule_name, "filter": filter}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateArchiveRule must succeed, got {created:?}"
        );

        let got = emulator.get_archive_rule("demo", rule_name).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: GetArchiveRule must succeed, got {got:?}"
        );
        let rule = got
            .member("archiveRule")
            .unwrap_or_else(|| panic!("case {case:?}: the response must carry `archiveRule`: {got:?}"));
        assert_eq!(
            rule.get("ruleName").and_then(Value::as_str),
            Some(*rule_name),
            "case {case:?}: ruleName must round-trip, got {:?}",
            rule.get("ruleName")
        );
        assert_eq!(
            rule.get("filter"),
            Some(filter),
            "case {case:?}: the filter must be exactly what create supplied, got {:?}",
            rule.get("filter")
        );
        assert!(
            is_timestamp(rule.get("createdAt")),
            "case {case:?}: createdAt must be the instant the rule was created, got {:?}",
            rule.get("createdAt")
        );
        assert!(
            is_timestamp(rule.get("updatedAt")),
            "case {case:?}: updatedAt must be an instant, got {:?}",
            rule.get("updatedAt")
        );
        assert_eq!(
            rule.get("updatedAt"),
            rule.get("createdAt"),
            "case {case:?}: a rule that was never updated reports its creation time, got {:?}",
            rule.get("updatedAt")
        );

        // A second create of the same rule name is a conflict, not an overwrite.
        let conflict = emulator
            .create_archive_rule("demo", json!({"ruleName": rule_name, "filter": filter}))
            .await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: a duplicate rule name must be a 409, got {conflict:?}"
        );
        assert_eq!(
            conflict.code(),
            "ConflictException",
            "case {case:?}: the 409 must be a ConflictException, got {conflict:?}"
        );

        rule_names.push((*rule_name).to_string());
    }

    rule_names.sort();
    let list = emulator.list_archive_rules("demo", "").await;
    assert_eq!(
        listed_rule_names(&list),
        rule_names,
        "ListArchiveRules must report every stored rule, got {list:?}"
    );

    // Paging over the rules follows the same rule as the analyzer listing.
    for page_size in [1_usize, 3, 4] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let reply = emulator.list_archive_rules("demo", &query).await;
            assert_eq!(
                reply.status, 200,
                "case page size {page_size}: a page must succeed, got {reply:?}"
            );
            let names = listed_rule_names(&reply);
            assert!(
                names.len() <= page_size,
                "case page size {page_size}: a page must hold at most {page_size} rules, got {names:?}"
            );
            seen.extend(names);
            token = reply
                .member("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, rule_names,
            "case page size {page_size}: paging must report every rule once, in order, got {seen:?}"
        );
    }

    // A repeated `clientToken` naming the same rule is the SAME create retried, so it
    // answers success — where the identical create WITHOUT the token conflicts.
    let tokened =
        json!({"ruleName": "tokened", "filter": {"isPublic": {"eq": ["true"]}}, "clientToken": "rule-token-1"});
    let first = emulator.create_archive_rule("demo", tokened.clone()).await;
    assert_eq!(
        first.status, 200,
        "the tokened rule create must succeed, got {first:?}"
    );
    for attempt in 1..=2 {
        let retry = emulator.create_archive_rule("demo", tokened.clone()).await;
        assert_eq!(
            retry.status, 200,
            "case retry {attempt}: a repeated clientToken must not conflict, got {retry:?}"
        );
    }
    let untokened = emulator
        .create_archive_rule(
            "demo",
            json!({"ruleName": "tokened", "filter": {"isPublic": {"eq": ["true"]}}}),
        )
        .await;
    assert_eq!(
        untokened.status, 409,
        "without the token the same rule name is a conflict, got {untokened:?}"
    );

    // An unknown rule on a known analyzer is a 404, as is any rule call on an
    // analyzer that does not exist.
    let not_found = [
        (
            "unknown rule",
            emulator.get_archive_rule("demo", "nope").await,
        ),
        (
            "unknown analyzer, GetArchiveRule",
            emulator.get_archive_rule("missing", "rule1").await,
        ),
        (
            "unknown analyzer, ListArchiveRules",
            emulator.list_archive_rules("missing", "").await,
        ),
        (
            "unknown analyzer, CreateArchiveRule",
            emulator
                .create_archive_rule(
                    "missing",
                    json!({"ruleName": "rule1", "filter": {"resourceType": {"eq": ["AWS::S3::Bucket"]}}}),
                )
                .await,
        ),
        ("unknown analyzer, GetAnalyzer", emulator.get_analyzer("missing").await),
    ];
    for (case, reply) in &not_found {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an absent resource must be a 404, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ResourceNotFoundException",
            "case {case:?}: the 404 must be a ResourceNotFoundException, got {reply:?}"
        );
    }
}

/// `CreateAnalyzer`'s inline `archiveRules` are created with the analyzer.
#[tokio::test]
async fn inline_archive_rules_are_stored_with_the_analyzer() {
    // (case, inline rules, expected rule names)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no inline rules", json!([]), vec![]),
        (
            "one inline rule",
            json!([{"ruleName": "only", "filter": {"resourceType": {"eq": ["AWS::S3::Bucket"]}}}]),
            vec!["only"],
        ),
        (
            "two inline rules",
            json!([
                {"ruleName": "second", "filter": {"isPublic": {"eq": ["true"]}}},
                {"ruleName": "first", "filter": {"error": {"exists": true}}}
            ]),
            vec!["first", "second"],
        ),
    ];

    for (case, rules, expected) in &cases {
        let emulator = Emulator::new();
        let create = emulator
            .create_analyzer(json!({
                "analyzerName": "inline",
                "type": "ACCOUNT",
                "archiveRules": rules,
            }))
            .await;
        assert_eq!(
            create.status, 200,
            "case {case:?}: the create must succeed, got {create:?}"
        );

        let list = emulator.list_archive_rules("inline", "").await;
        assert_eq!(
            list.status, 200,
            "case {case:?}: ListArchiveRules must succeed, got {list:?}"
        );
        assert_eq!(
            listed_rule_names(&list),
            *expected,
            "case {case:?}: the inline rules must be stored, got {:?}",
            listed_rule_names(&list)
        );
        for rule_name in expected {
            let got = emulator.get_archive_rule("inline", rule_name).await;
            assert_eq!(
                got.status, 200,
                "case {case:?}: inline rule {rule_name:?} must be readable, got {got:?}"
            );
        }
    }
}

/// A delete is observable by the next call: the analyzer, and the rules under it,
/// are gone.
#[tokio::test]
async fn deleting_an_analyzer_removes_it_and_its_archive_rules() {
    let emulator = Emulator::new();
    for name in ["keep", "drop"] {
        let create = emulator
            .create_analyzer(json!({
                "analyzerName": name,
                "type": "ACCOUNT",
                "archiveRules": [{"ruleName": "rule1", "filter": {"isPublic": {"eq": ["true"]}}}],
            }))
            .await;
        assert_eq!(
            create.status, 200,
            "case {name:?}: the fixture create must succeed, got {create:?}"
        );
    }

    let deleted = emulator.delete_analyzer("drop").await;
    assert_eq!(deleted.status, 200, "DeleteAnalyzer must succeed: {deleted:?}");

    // Every later call agrees the analyzer is gone.
    let after: Vec<(&str, Reply)> = vec![
        ("GetAnalyzer", emulator.get_analyzer("drop").await),
        (
            "GetArchiveRule",
            emulator.get_archive_rule("drop", "rule1").await,
        ),
        (
            "ListArchiveRules",
            emulator.list_archive_rules("drop", "").await,
        ),
        ("DeleteAnalyzer again", emulator.delete_analyzer("drop").await),
        ("DeleteAnalyzer of a name never created", emulator.delete_analyzer("never").await),
    ];
    for (case, reply) in &after {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: the deleted analyzer must be absent, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ResourceNotFoundException",
            "case {case:?}: the 404 must be a ResourceNotFoundException, got {reply:?}"
        );
    }

    let list = emulator.list_analyzers("").await;
    assert_eq!(
        listed_names(&list),
        vec!["keep".to_string()],
        "the listing must drop the deleted analyzer, got {list:?}"
    );

    // The name is free again, and the new analyzer starts with no archive rules.
    let recreated = emulator
        .create_analyzer(json!({"analyzerName": "drop", "type": "ORGANIZATION"}))
        .await;
    assert_eq!(
        recreated.status, 200,
        "re-creating a deleted name must succeed, got {recreated:?}"
    );
    let rules = emulator.list_archive_rules("drop", "").await;
    assert_eq!(
        listed_rule_names(&rules),
        Vec::<String>::new(),
        "the re-created analyzer must carry none of the deleted one's rules, got {rules:?}"
    );
    let get = emulator.get_analyzer("drop").await;
    assert_eq!(
        get.member("analyzer")
            .and_then(|analyzer| analyzer.get("type"))
            .and_then(Value::as_str),
        Some("ORGANIZATION"),
        "the re-created analyzer must be the one just created, got {get:?}"
    );
}

/// No single listing hands back the whole store: the page is capped server-side, and
/// the rest is reached by following `nextToken`.
#[tokio::test]
async fn a_listing_never_returns_more_than_the_page_cap() {
    let emulator = Emulator::new();
    // A few more analyzers than fit on one page. Names are zero-padded so their name
    // order (the order the listing pages in) is their creation order.
    let overflow = 5_usize;
    let total = MAX_PAGE_SIZE as usize + overflow;
    for index in 0..total {
        let reply = emulator
            .create_analyzer(json!({"analyzerName": format!("a{index:04}"), "type": "ACCOUNT"}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case fixture {index}: the create must succeed, got {reply:?}"
        );
    }

    // (case, query) — an absent `maxResults` and one larger than the cap both yield a
    // capped page, never the whole store.
    let capped = [
        ("no maxResults", String::new()),
        ("maxResults above the cap", format!("?maxResults={}", MAX_PAGE_SIZE + 1)),
        ("maxResults far above the cap", "?maxResults=100000".to_string()),
    ];
    for (case, query) in &capped {
        let reply = emulator.list_analyzers(query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the listing must succeed, got {reply:?}"
        );
        let names = listed_names(&reply);
        assert_eq!(
            names.len(),
            MAX_PAGE_SIZE as usize,
            "case {case:?}: the page must be capped at {MAX_PAGE_SIZE}, got {} items",
            names.len()
        );
        let token = reply
            .member("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| panic!("case {case:?}: a capped page must carry a nextToken: {reply:?}"));

        // The rest is reachable, exactly once, from the token the capped page gave.
        let rest = emulator
            .list_analyzers(&format!("?nextToken={token}"))
            .await;
        assert_eq!(
            listed_names(&rest).len(),
            overflow,
            "case {case:?}: the remainder must hold the other {overflow}, got {:?}",
            listed_names(&rest)
        );
        assert!(
            rest.member("nextToken").is_none(),
            "case {case:?}: the last page must carry no token, got {rest:?}"
        );
    }
}

/// The analyzer store is bounded: past the ceiling the create is refused with the
/// quota error the operation models, not by growing the store.
#[tokio::test]
async fn the_analyzer_store_is_bounded() {
    let emulator = Emulator::new();
    for index in 0..MAX_ANALYZERS {
        let reply = emulator
            .create_analyzer(json!({"analyzerName": format!("a{index:04}"), "type": "ACCOUNT"}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case analyzer {index}: a create below the ceiling must succeed, got {reply:?}"
        );
    }

    let refused = emulator
        .create_analyzer(json!({"analyzerName": "oneTooMany", "type": "ACCOUNT"}))
        .await;
    assert_eq!(
        refused.status, 402,
        "a create past the ceiling must be refused with the modelled quota error, got {refused:?}"
    );
    assert_eq!(
        refused.code(),
        "ServiceQuotaExceededException",
        "the refusal must be a ServiceQuotaExceededException, got {refused:?}"
    );
    let absent = emulator.get_analyzer("oneTooMany").await;
    assert_eq!(
        absent.status, 404,
        "a refused create must store nothing, got {absent:?}"
    );

    // Deleting one frees exactly one slot, so the store is a ceiling and not a
    // one-way ratchet.
    let deleted = emulator.delete_analyzer("a0000").await;
    assert_eq!(deleted.status, 200, "the delete must succeed, got {deleted:?}");
    let accepted = emulator
        .create_analyzer(json!({"analyzerName": "roomAgain", "type": "ACCOUNT"}))
        .await;
    assert_eq!(
        accepted.status, 200,
        "a create into the freed slot must succeed, got {accepted:?}"
    );
}

/// The per-analyzer archive-rule store is bounded: past the ceiling the create is
/// refused with the quota error the operation models, not by growing the store.
#[tokio::test]
async fn the_archive_rule_store_is_bounded() {
    let emulator = Emulator::new();
    let created = emulator
        .create_analyzer(json!({"analyzerName": "full", "type": "ACCOUNT"}))
        .await;
    assert_eq!(created.status, 200, "the analyzer create must succeed: {created:?}");

    let filter = json!({"isPublic": {"eq": ["true"]}});
    for index in 0..MAX_ARCHIVE_RULES_PER_ANALYZER {
        let reply = emulator
            .create_archive_rule(
                "full",
                json!({"ruleName": format!("r{index:04}"), "filter": filter}),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case rule {index}: a create below the ceiling must succeed, got {reply:?}"
        );
    }

    let refused = emulator
        .create_archive_rule(
            "full",
            json!({"ruleName": "oneTooMany", "filter": filter}),
        )
        .await;
    assert_eq!(
        refused.status, 402,
        "a create past the ceiling must be refused with the modelled quota error, got {refused:?}"
    );
    assert_eq!(
        refused.code(),
        "ServiceQuotaExceededException",
        "the refusal must be a ServiceQuotaExceededException, got {refused:?}"
    );

    // The refusal stored nothing: the rule the ceiling refused is not readable.
    let absent = emulator.get_archive_rule("full", "oneTooMany").await;
    assert_eq!(
        absent.status, 404,
        "a refused create must store nothing, got {absent:?}"
    );
}
