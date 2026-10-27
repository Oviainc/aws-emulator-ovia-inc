// SPDX-License-Identifier: CC0-1.0

//! What a `@pattern` refusal REPORTS, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and this service's refusal wording all take part, exactly as they do for an SDK
//! client. The harness is the one `collection_lifecycle` owns; nothing is rebuilt here.
//!
//! The rule under test: a constraint-violation clause reports the regular expression the
//! SERVICE's own validator applies to the violated member — the modelled `@pattern` without
//! the `^`/`$` anchors a pattern clause already implies, and, for a member typed
//! `CollectionName`, the character class the collection-name validator reports, which
//! admits `_`. What the clause reports is not what the service accepts: which values a
//! member may hold stays the modelled trait, so no row here asserts an acceptance.
//!
//! The table spans what the rule generalises over: five constrained shapes
//! (`CollectionName`, `CollectionGroupName`, `PolicyName`, `PolicyVersion`, `CollectionId`,
//! `VpcId`), a direct member, a list element at two positions and a member nested in a
//! structure, two members of ONE operation that report different classes, and operations
//! whose clause is the trait and operations whose clause is not — not the one request the
//! issue quoted. Every other constraint class keeps the framework's own clause, and the
//! operations that report a collection name still serve a request the model accepts.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// A `data` access-policy document the handlers accept.
const EMPTY_RULES: &str = r#"{"Rules":[]}"#;

/// The class this service's own collection-name validator reports, from a recorded refusal
/// of `CreateCollection`: it admits `_`, which `CollectionName`'s modelled
/// `^[a-z][a-z0-9-]+$` does not.
const COLLECTION_NAME_CLASS: &str = "[a-z][a-z0-9_-]+";

/// The modelled `@pattern` of `CollectionName`, `CollectionGroupName` and `PolicyName`
/// alike (`smithy_query(query="shape", shape="CollectionName,CollectionGroupName,PolicyName",
/// model_path="services/service-com-amazonaws-opensearchserverless-opensearchserverless/smithy-model/com-amazonaws-opensearchserverless-opensearchserverless.json")`),
/// anchors and all — one string for three shapes, which is why a collection name is told
/// apart by the member it sits at.
const MODELLED_NAME_PATTERN: &str = "^[a-z][a-z0-9-]+$";

/// One row: the call that violates a `@pattern`, and the clause it must state.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// The dotted member path the clause names.
    path: &'static str,
    /// The value the clause echoes.
    value: &'static str,
    /// The regular expression the clause reports.
    reported: &'static str,
    /// The modelled `@pattern` of the violated member, which must NOT appear in the clause:
    /// its anchors are the whole difference for every row but a collection name's.
    modelled: &'static str,
}

/// The envelope one violation is stated in, which is the framework's own.
fn one_violation(value: &str, path: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {pattern}"
    )
}

#[tokio::test]
async fn a_pattern_refusal_reports_the_regex_the_services_validator_applies() {
    let router = emulator();

    // The stored policy the `policyVersion` row updates; its own name is well inside
    // `PolicyName`, so that call is refused on the version alone.
    let (status, _, created) = call(
        &router,
        "CreateAccessPolicy",
        json!({ "type": "data", "name": "stored-access", "policy": EMPTY_RULES }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateAccessPolicy(stored-access): expected 200, got {status} with {created}"
    );

    let rows: Vec<Row> = vec![
        // ---- a member typed `CollectionName`: the validator's class, not the trait ----
        Row {
            case: "a collection name starting with a digit",
            operation: "CreateCollection",
            request: json!({ "name": "1invalid-name" }),
            path: "name",
            value: "1invalid-name",
            reported: COLLECTION_NAME_CLASS,
            modelled: MODELLED_NAME_PATTERN,
        },
        Row {
            case: "a collection name holding an uppercase letter",
            operation: "CreateCollection",
            request: json!({ "name": "Upper-Collection" }),
            path: "name",
            value: "Upper-Collection",
            reported: COLLECTION_NAME_CLASS,
            modelled: MODELLED_NAME_PATTERN,
        },
        Row {
            case: "the first element of BatchGetCollection's names list",
            operation: "BatchGetCollection",
            request: json!({ "names": ["1bad-name"] }),
            path: "names.1.member",
            value: "1bad-name",
            reported: COLLECTION_NAME_CLASS,
            modelled: MODELLED_NAME_PATTERN,
        },
        Row {
            case: "a later element of BatchGetCollection's names list",
            operation: "BatchGetCollection",
            request: json!({ "names": ["good-name", "bad name"] }),
            path: "names.2.member",
            value: "bad name",
            reported: COLLECTION_NAME_CLASS,
            modelled: MODELLED_NAME_PATTERN,
        },
        Row {
            case: "the collection name nested in ListCollections' filters",
            operation: "ListCollections",
            request: json!({ "collectionFilters": { "name": "1bad-filter" } }),
            path: "collectionFilters.name",
            value: "1bad-filter",
            reported: COLLECTION_NAME_CLASS,
            modelled: MODELLED_NAME_PATTERN,
        },
        // ---- every other member: the trait, with its anchors dropped ----
        Row {
            case: "the collection GROUP name beside CreateCollection's own name",
            operation: "CreateCollection",
            request: json!({ "name": "valid-collection", "collectionGroupName": "1bad-group" }),
            path: "collectionGroupName",
            value: "1bad-group",
            reported: "[a-z][a-z0-9-]+",
            modelled: MODELLED_NAME_PATTERN,
        },
        Row {
            case: "a collection group's own name",
            operation: "CreateCollectionGroup",
            request: json!({ "name": "1cd", "standbyReplicas": "ENABLED" }),
            path: "name",
            value: "1cd",
            reported: "[a-z][a-z0-9-]+",
            modelled: MODELLED_NAME_PATTERN,
        },
        Row {
            case: "a policy name",
            operation: "CreateAccessPolicy",
            request: json!({ "type": "data", "name": "1bad-policy", "policy": EMPTY_RULES }),
            path: "name",
            value: "1bad-policy",
            reported: "[a-z][a-z0-9-]+",
            modelled: MODELLED_NAME_PATTERN,
        },
        Row {
            case: "a policy version",
            operation: "UpdateAccessPolicy",
            request: json!({
                "type": "data",
                "name": "stored-access",
                "policyVersion": "AAAAAAAAAAAAAAAAAAAA==",
            }),
            path: "policyVersion",
            value: "AAAAAAAAAAAAAAAAAAAA==",
            reported: "([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?",
            modelled: "^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$",
        },
        Row {
            case: "a collection id in the ids list of the SAME operation as the names list",
            operation: "BatchGetCollection",
            request: json!({ "ids": ["Not-A-Collection-Id"] }),
            path: "ids.1.member",
            value: "Not-A-Collection-Id",
            reported: "[a-z0-9]{3,40}",
            modelled: "^[a-z0-9]{3,40}$",
        },
        Row {
            case: "a VPC id, whose class is neither a name's nor an id's",
            operation: "CreateVpcEndpoint",
            request: json!({
                "name": "my-endpoint",
                "vpcId": "vpc_BAD",
                "subnetIds": ["subnet-12345678"],
            }),
            path: "vpcId",
            value: "vpc_BAD",
            reported: "vpc-[0-9a-z]*",
            modelled: "^vpc-[0-9a-z]*$",
        },
    ];

    for Row {
        case,
        operation,
        request,
        path,
        value,
        reported,
        modelled,
    } in rows
    {
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case:?}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
        let stated = text(&answer, "message");
        assert_eq!(
            stated,
            one_violation(value, path, reported),
            "case {case:?}: expected the clause to report {reported:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(modelled),
            "case {case:?}: the clause must not report the modelled {modelled:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn every_other_constraint_class_keeps_the_frameworks_clause() {
    let router = emulator();

    // (case, operation, request, the clause the framework states)
    //
    // The rows run through the operations that report a collection name, so the one thing
    // this service restates is the pattern clause and nothing else about a refusal.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a collection name one short of the minimum",
            "CreateCollection",
            json!({ "name": "ab" }),
            "Value 'ab' at 'name' failed to satisfy constraint: Member must have length \
             greater than or equal to 3",
        ),
        (
            "a required member absent",
            "CreateCollection",
            json!({ "description": "no name" }),
            "Value null at 'name' failed to satisfy constraint: Member must not be null",
        ),
        (
            "an empty names list",
            "BatchGetCollection",
            json!({ "names": [] }),
            "Value '0 items' at 'names' failed to satisfy constraint: Member must have length \
             greater than or equal to 1",
        ),
        (
            "a page bound below the range",
            "ListCollections",
            json!({ "maxResults": 0 }),
            "Value '0' at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1",
        ),
        (
            "a page bound above the range",
            "ListCollections",
            json!({ "maxResults": 101 }),
            "Value '101' at 'maxResults' failed to satisfy constraint: Member must have value \
             less than or equal to 100",
        ),
    ];

    for (case, operation, request, clause) in rows {
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case:?}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
        let stated = text(&answer, "message");
        assert_eq!(
            stated,
            format!("1 validation error detected: {clause}"),
            "case {case:?}: the framework's own clause is kept, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn the_operations_that_report_a_collection_name_still_serve_what_the_model_accepts() {
    let router = emulator();

    // Names differing along `CollectionName`'s @length(min: 3, max: 64) and its character
    // class: the minimum, a mixed letter/digit/hyphen name, and the maximum.
    let longest = "a".repeat(64);
    let names: [&str; 3] = ["abc", "a-b-c-1-2-3", longest.as_str()];

    for name in names {
        let (status, code, answer) = call(
            &router,
            "CreateCollection",
            json!({ "name": name, "encryptionConfig": { "aWSOwnedKey": true } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateCollection({name}): expected 200, got {status} {code} with {answer}"
        );
        let created = text(&answer, "createCollectionDetail.name");
        assert_eq!(
            created, name,
            "case CreateCollection({name}): expected the stored name {name:?}, got {created:?}"
        );

        // Read back through the two other operations the refusal wrapper serves, so a
        // request the model accepts reaches the handler and the one shared store.
        let (status, code, read) = call(&router, "BatchGetCollection", json!({ "names": [name] })).await;
        assert_eq!(
            status, 200,
            "case BatchGetCollection({name}): expected 200, got {status} {code} with {read}"
        );
        assert_eq!(
            text(at(&read, "collectionDetails").get(0).unwrap_or(&Value::Null), "name"),
            name,
            "case BatchGetCollection({name}): expected the created collection, got {read}"
        );

        let (status, code, listed) = call(
            &router,
            "ListCollections",
            json!({ "collectionFilters": { "name": name }, "maxResults": 100 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case ListCollections({name}): expected 200, got {status} {code} with {listed}"
        );
        let summaries = at(&listed, "collectionSummaries")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            1,
            "case ListCollections({name}): expected exactly the filtered collection, got {listed}"
        );
        assert_eq!(
            text(&summaries[0], "name"),
            name,
            "case ListCollections({name}): expected the created collection, got {listed}"
        );
    }
}
