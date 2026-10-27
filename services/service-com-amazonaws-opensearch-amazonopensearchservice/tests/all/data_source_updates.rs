// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDataSource`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the data-source calls from
//! [`super::data_source_registry`]; this suite adds no second copy of either.
//!
//! The rule under test: an update REPLACES exactly the members the request carries on
//! the data source the domain holds under the name it addresses, leaves the members it
//! carries none for as they were, and is observable through every later read of that
//! record; a name the store holds no domain under has no data source to modify
//! (`ResourceNotFoundException`), and a name the domain holds no data source under is
//! a value that could not be resolved (`ValidationException`, the refusal the read
//! path answers such a name with). The operation models exactly `BaseException` 400,
//! `DependencyFailureException` 424, `DisabledOperationException` 409,
//! `InternalException` 500, `ResourceNotFoundException` 409 and `ValidationException`
//! 400 (`smithy_query(query="operation",operation="UpdateDataSource")`).
//!
//! The assertions span a SET of inputs rather than one call: every `DataSourceStatus`
//! the model declares, a description supplied and withheld, and a `DataSourceType`
//! whose catalog role changes — each read back through `GetDataSource` AND the
//! listing. The scope of ONE update is asserted the same way: over every (domain,
//! data source) pair of a store holding the same names on several domains, the
//! addressed record changes and every other one keeps what it was last left at.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::DATA_SOURCE_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::DATA_SOURCE_STATUS_ACTIVE;

use super::data_source_registry::{add, created_domain, data_sources, get};
use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// `UpdateDataSource`: `PUT /opensearch/domain/{DomainName}/dataSource/{Name}`.
async fn update(emulator: &Emulator, domain: &str, name: &str, body: Value) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/domain/{domain}/dataSource/{name}"),
            Some(body),
        )
        .await
}

/// The `S3GlueDataCatalog` data source type naming `role_arn`.
fn data_source_type(role_arn: &str) -> Value {
    json!({ "S3GlueDataCatalog": { "RoleArn": role_arn } })
}

/// An IAM role ARN of the form `S3GlueDataCatalog$RoleArn` takes.
fn role(suffix: &str) -> String {
    format!("arn:aws:iam::000000000000:role/service-role/data-source-{suffix}")
}

/// The four members a read reports for one data source: its name, type, description
/// and status.
fn summary(details: &Value) -> (Option<&str>, &Value, Option<&str>, Option<&str>) {
    (
        details["Name"].as_str(),
        &details["DataSourceType"],
        details["Description"].as_str(),
        details["Status"].as_str(),
    )
}

/// The `GetDataSource` answer for one data source, as the four-member summary above.
async fn read_back(emulator: &Emulator, domain: &str, name: &str, case: &str) -> Value {
    let answer = get(emulator, domain, name).await;
    assert_eq!(
        answer.status, 200,
        "case GetDataSource({case}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body.clone()
}

#[tokio::test]
async fn an_update_replaces_exactly_the_members_the_request_carries() {
    let emulator = Emulator::new();
    let domain = "catalog-updates";
    created_domain(&emulator, domain).await;

    let name = "sales_catalog";
    let created = add(
        &emulator,
        domain,
        json!({
            "Name": name,
            "DataSourceType": data_source_type(&role("created")),
            "Description": "the catalog as created",
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case AddDataSource: expected 200, got {} with {:?}",
        created.status, created.body
    );

    // Rows differ along what the rule generalises over: which members the request
    // carries (a description, a status, both, neither) and what it sets them to —
    // every value of `DataSourceStatus` the model declares
    // (`smithy_query(query="shape", shape="DataSourceStatus")`, exposed as
    // `DATA_SOURCE_STATUS_VALUES`) plus the type's own role, which changes per row so
    // a replaced `DataSourceType` is observable too.
    //
    // Each row states the WHOLE record it expects afterwards, so a member the request
    // did not carry is asserted to have kept the value the row before it left.
    let mut expected_description = "the catalog as created".to_string();
    let mut expected_status = DATA_SOURCE_STATUS_ACTIVE.to_string();
    let mut rows: Vec<(String, Option<String>, Option<String>)> = vec![
        ("a new description only".to_string(), Some("now reading the lake".to_string()), None),
    ];
    for status in DATA_SOURCE_STATUS_VALUES {
        rows.push((
            format!("{status} with a description"),
            Some(format!("the catalog while {status}")),
            Some(status.to_string()),
        ));
        rows.push((format!("{status} and nothing else"), None, Some(status.to_string())));
    }
    rows.push(("neither member".to_string(), None, None));

    for (index, (case, description, status)) in rows.into_iter().enumerate() {
        let role_arn = role(&index.to_string());
        let mut body = json!({ "DataSourceType": data_source_type(&role_arn) });
        if let Some(description) = description.clone() {
            body["Description"] = json!(description);
            expected_description = description;
        }
        if let Some(status) = status.clone() {
            body["Status"] = json!(status);
            expected_status = status;
        }

        let answer = update(&emulator, domain, name, body).await;
        assert_eq!(
            answer.status, 200,
            "case UpdateDataSource({case}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let message = answer.body["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(name) && message.contains(domain),
            "case UpdateDataSource({case}): the message must name the data source and its \
             domain, got {message:?}"
        );

        // The answer is about the STORED record, so every read path reports what the
        // update left: the members it carried replaced, the rest untouched.
        let expected = (
            Some(name),
            &data_source_type(&role_arn),
            Some(expected_description.as_str()),
            Some(expected_status.as_str()),
        );
        let read = read_back(&emulator, domain, name, &case).await;
        assert_eq!(
            summary(&read),
            expected,
            "case GetDataSource({case}): expected {expected:?}, got {read:?}"
        );
        let listed = super::data_source_registry::list(&emulator, domain).await;
        let listed = data_sources(&listed, &case);
        assert_eq!(
            listed.len(),
            1,
            "case ListDataSources({case}): an update must not add a record, got {listed:?}"
        );
        let entry = listed.first().expect("the listing holds the one record");
        assert_eq!(
            summary(entry),
            expected,
            "case ListDataSources({case}): expected {expected:?}, got {entry:?}"
        );
    }
}

#[tokio::test]
async fn an_update_reaches_only_the_record_it_addresses() {
    let emulator = Emulator::new();

    // Several domains, each holding data sources under the SAME names: a data source
    // belongs to one domain and is keyed within it by name, so an update that reached
    // another domain's record, or a sibling of the one it names, is observable here.
    let domains = ["scope-alpha", "scope-beta", "scope-gamma"];
    let names = ["first_catalog", "second_catalog", "third_catalog"];

    // The whole record expected of each (domain, name): the role suffix its
    // `DataSourceType` carries, its description and its status.
    let mut expected: BTreeMap<(&str, &str), (String, String, String)> = BTreeMap::new();
    for domain in domains {
        created_domain(&emulator, domain).await;
        for name in names {
            let role_suffix = format!("{domain}-{name}-created");
            let description = format!("{name} of {domain} as created");
            let answer = add(
                &emulator,
                domain,
                json!({
                    "Name": name,
                    "DataSourceType": data_source_type(&role(&role_suffix)),
                    "Description": description,
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case AddDataSource({domain}/{name}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            expected.insert(
                (domain, name),
                (
                    role_suffix,
                    description,
                    DATA_SOURCE_STATUS_ACTIVE.to_string(),
                ),
            );
        }
    }

    // One row per (domain, name) pair: that pair is the target of ONE update, whose
    // status cycles through every value the model declares. After each row the WHOLE
    // store is re-read, so a row only passes when the addressed record carries the
    // update and every other record still carries what the row before it left.
    let targets = domains
        .iter()
        .flat_map(|domain| names.iter().map(move |name| (*domain, *name)));
    for (index, (domain, name)) in targets.enumerate() {
        let case = format!("{domain}/{name}");
        let role_suffix = format!("{domain}-{name}-updated");
        let description = format!("{name} of {domain} after update {index}");
        let status = DATA_SOURCE_STATUS_VALUES[index % DATA_SOURCE_STATUS_VALUES.len()];
        let answer = update(
            &emulator,
            domain,
            name,
            json!({
                "DataSourceType": data_source_type(&role(&role_suffix)),
                "Description": description,
                "Status": status,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case UpdateDataSource({case}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        expected.insert((domain, name), (role_suffix, description, status.to_string()));

        for other_domain in domains {
            let listing = super::data_source_registry::list(&emulator, other_domain).await;
            let listed = data_sources(&listing, &case);
            assert_eq!(
                listed.len(),
                names.len(),
                "case ListDataSources({other_domain} after {case}): expected {} records, got \
                 {listed:?}",
                names.len()
            );
            for other_name in names {
                let (role_suffix, description, status) = expected
                    .get(&(other_domain, other_name))
                    .expect("every created data source has an expected record");
                let expected_type = data_source_type(&role(role_suffix));
                let want = (
                    Some(other_name),
                    &expected_type,
                    Some(description.as_str()),
                    Some(status.as_str()),
                );
                let read = read_back(&emulator, other_domain, other_name, &case).await;
                assert_eq!(
                    summary(&read),
                    want,
                    "case GetDataSource({other_domain}/{other_name} after {case}): expected \
                     {want:?}, got {read:?}"
                );
                let entry = listed
                    .iter()
                    .find(|entry| entry["Name"].as_str() == Some(other_name))
                    .unwrap_or_else(|| {
                        panic!(
                            "case ListDataSources({other_domain} after {case}): the listing must \
                             report {other_name}, got {listed:?}"
                        )
                    });
                assert_eq!(
                    summary(entry),
                    want,
                    "case ListDataSources({other_domain}/{other_name} after {case}): expected \
                     {want:?}, got {entry:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_update_naming_nothing_the_account_holds_changes_nothing() {
    let emulator = Emulator::new();
    let domain = "catalog-refusals";
    created_domain(&emulator, domain).await;
    let name = "held_catalog";
    add(
        &emulator,
        domain,
        json!({
            "Name": name,
            "DataSourceType": data_source_type(&role("held")),
            "Description": "the record a refused update must not touch",
        }),
    )
    .await;
    let before = read_back(&emulator, domain, name, "before the refusals").await;

    // Rows: the domain exists but the data source does not; the domain does not exist
    // at all. The two carry DIFFERENT modelled errors, which is how a caller tells
    // "no such domain" from "no such data source on this domain" — the same split the
    // read path answers with.
    for (case, target_domain, target_name, expected) in [
        (
            "a data source the domain does not hold",
            domain,
            "absent_catalog",
            "ValidationException",
        ),
        (
            "a domain the store does not hold",
            "no-such-domain",
            name,
            "ResourceNotFoundException",
        ),
    ] {
        let answer = update(
            &emulator,
            target_domain,
            target_name,
            json!({
                "DataSourceType": data_source_type(&role("refused")),
                "Description": "an update that must not land",
                "Status": "DISABLED",
            }),
        )
        .await;
        assert_eq!(
            error_code(&answer),
            expected,
            "case UpdateDataSource({case}): expected {expected}, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    let after = read_back(&emulator, domain, name, "after the refusals").await;
    assert_eq!(
        summary(&after),
        summary(&before),
        "case GetDataSource(after the refusals): a refused update must change nothing"
    );
}
