// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the request rules `AddDataSource` answers by, over the wire.
//!
//! [`super::data_source_registry`] pins what the registry STORES and reports; this
//! suite pins the two rules a create is judged by BEFORE the store is consulted, on
//! the members that suite leaves at values well inside their constraints:
//!
//! * a create whose request omits a member the model marks required is refused as
//!   invalid (HTTP 400 `ValidationException`) and persists nothing;
//! * a create is accepted exactly where the modelled constraint of the member it
//!   carries allows the value — `Description` at its published length bounds and one
//!   past, over the character classes its pattern publishes, and `DomainName` at its
//!   published bounds and one past each, plus one row per shape its pattern refuses —
//!   and a refused create leaves the domain's stored set exactly as it was.
//!
//! The wire harness comes from [`super::domain_lifecycle`] and the data-source call
//! helpers from [`super::data_source_registry`]; this suite adds no second copy of
//! either.

use serde_json::{json, Value};

use super::data_source_registry::{add, created_domain, data_sources, get, list};
use super::domain_lifecycle::{error_code, Emulator};

/// The names the listing of `domain` reports, with the case named on failure.
async fn listed_names(emulator: &Emulator, domain: &str, case: &str) -> Vec<String> {
    data_sources(&list(emulator, domain).await, case)
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The union value every row below carries, so the only thing a row varies is the
/// member under test. `DataSourceType` declares exactly one member,
/// `S3GlueDataCatalog`
/// (`smithy_query(query="shape",shape="DataSourceType")`).
fn data_source_type() -> Value {
    json!({ "S3GlueDataCatalog": { "RoleArn": "arn:aws:iam::123456789012:role/s3-glue-rules" } })
}

#[tokio::test]
async fn a_create_missing_a_required_member_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    created_domain(&emulator, "required-members").await;

    // `AddDataSource` marks `Name` and `DataSourceType` required, beside the
    // `DomainName` the URI carries
    // (`smithy_query(query="operation",operation="AddDataSource")`). One row per
    // omission, and one omitting both.
    let rows: Vec<(&str, Value)> = vec![
        (
            "no Name",
            json!({ "DataSourceType": data_source_type(), "Description": "a description" }),
        ),
        ("no DataSourceType", json!({ "Name": "missing_type" })),
        ("neither", json!({ "Description": "a description" })),
        // An empty body omits every member of the body, which is the same refusal.
        ("empty body", json!({})),
    ];

    for (case, body) in rows {
        let answer = add(&emulator, "required-members", body).await;
        assert_eq!(
            answer.status, 400,
            "case AddDataSource({case}): expected 400 for a missing required member, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case AddDataSource({case}): expected ValidationException, got {:?}",
            answer.body
        );
        let names = listed_names(&emulator, "required-members", case).await;
        assert!(
            names.is_empty(),
            "case AddDataSource({case}): a refused create must persist nothing, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_description_is_accepted_exactly_where_the_modelled_constraint_allows_it() {
    let emulator = Emulator::new();
    created_domain(&emulator, "description-rules").await;

    // `DataSourceDescription` is `@length(min: 0, max: 1000)`: both published bounds
    // and one past the upper one. The accepted rows also span the character classes
    // the member's pattern publishes — alphanumerics and `_` first, then the
    // punctuation class `@#%*+=:?./!`, whitespace and `-`
    // (`smithy_query(query="shape",shape="DataSourceDescription")`).
    let max_length = "d".repeat(1000);
    let past_max = "d".repeat(1001);
    let rows: Vec<(&str, &str, bool)> = vec![
        ("empty, the published minimum", "", true),
        ("one character", "d", true),
        ("the published maximum", max_length.as_str(), true),
        ("one past the published maximum", past_max.as_str(), false),
        (
            "the published punctuation classes",
            "ops_team@example.com: 50% +1/2 path/to/file.json!",
            true,
        ),
        ("whitespace and a hyphen", "primary s3 glue - catalog", true),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (index, (case, description, allowed)) in rows.iter().enumerate() {
        let name = format!("desc_row_{index}");
        let answer = add(
            &emulator,
            "description-rules",
            json!({
                "Name": name,
                "DataSourceType": data_source_type(),
                "Description": description,
            }),
        )
        .await;
        if *allowed {
            assert_eq!(
                answer.status, 200,
                "case AddDataSource({case}): expected 200 inside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            accepted.push(name.clone());
            // Accepted means PERSISTED VERBATIM: the description reads back exactly
            // as sent, whatever its length.
            let read = get(&emulator, "description-rules", &name).await;
            assert_eq!(
                read.body["Description"],
                json!(description),
                "case GetDataSource({case}): expected the description as sent, got {:?}",
                read.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case AddDataSource({case}): expected 400 outside the constraint, got {} with \
                 {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case AddDataSource({case}): expected ValidationException, got {:?}",
                answer.body
            );
        }

        // Whatever the row, the stored set is exactly the rows accepted so far: a
        // refusal adds nothing and changes nothing already there.
        let mut names = listed_names(&emulator, "description-rules", case).await;
        names.sort();
        let mut expected = accepted.clone();
        expected.sort();
        assert_eq!(
            names, expected,
            "case AddDataSource({case}): expected the stored set {expected:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_domain_name_is_accepted_exactly_where_the_modelled_constraint_allows_it() {
    let emulator = Emulator::new();

    // `DomainName` is `@length(min: 3, max: 28)` with `@pattern("^[a-z][a-z0-9\-]+$")`
    // (`smithy_query(query="operation",operation="AddDataSource")`): each published
    // bound and one past it, plus one row per character class the pattern refuses.
    // `AddDataSource` carries it as an HTTP label, so the rule holds on the URI too.
    let longest = format!("d{}", "x".repeat(27));
    let past_longest = format!("d{}", "x".repeat(28));
    let rows: Vec<(&str, bool)> = vec![
        ("abc", true),
        (longest.as_str(), true),
        // One past each published length bound.
        ("ab", false),
        (past_longest.as_str(), false),
        // The pattern refuses an upper-case letter, a leading digit, an underscore
        // and a dot.
        ("Abc", false),
        ("0abc", false),
        ("a_bc", false),
        ("a.bc", false),
    ];

    for (domain, allowed) in rows {
        let case = format!("AddDataSource({domain:?})");
        let body = json!({ "Name": "label_row", "DataSourceType": data_source_type() });
        if allowed {
            // Inside the constraint the create reaches the store, so it is accepted
            // on a domain that exists.
            created_domain(&emulator, domain).await;
            let answer = add(&emulator, domain, body).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 inside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            let names = listed_names(&emulator, domain, &case).await;
            assert_eq!(
                names,
                vec!["label_row".to_string()],
                "case {case}: expected the created record to be stored, got {names:?}"
            );
        } else {
            // Outside it the request never reaches the store: it is refused as
            // invalid, not reported as a domain that does not exist.
            let answer = add(&emulator, domain, body).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400 outside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {:?}",
                answer.body
            );
        }
    }
}
