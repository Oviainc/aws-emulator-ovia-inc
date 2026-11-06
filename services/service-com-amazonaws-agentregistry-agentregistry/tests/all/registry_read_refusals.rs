// SPDX-License-Identifier: CC0-1.0

//! How the three Agent Registry discovery reads WORD a refusal, end-to-end over the
//! same `MultiServiceRouter::dispatch` the emulator serves on its socket.
//!
//! Three rules, each asserted over a set of inputs rather than one:
//!
//! 1. a request naming a registry this service does not hold is refused with
//!    `404 ResourceNotFoundException` and one sentence — the fixed text plus the
//!    registry identifier exactly as the request supplied it. The per-entry `errors`
//!    list of `BatchGetDiscoverableRegistryRecord` is for record ids a registry that
//!    EXISTS does not expose, which the contrast rows pin;
//! 2. no constraint clause quotes a value: each names the member it was stated at and
//!    the requirement that member's value had to satisfy, whatever the request carried
//!    — over a collection's `@length`, a scalar's `@pattern` and `@range`, a string's
//!    `@length` and an absent `@required` member;
//! 3. a constraint the ELEMENTS of a collection declare is stated at that MEMBER with
//!    the element shape's whole declared set nested inside the clause, and a `@pattern`
//!    inside or outside that set is quoted as the service advertises the expression:
//!    without the pair of whole-value anchors the model wraps it in.
//!
//! The last test pins the other direction: a value AT each published bound is served,
//! so the refusals above are about values outside the model's bounds.
//!
//! Every bound, pattern and member path below comes from the model
//! (`smithy_query(query="closure", operation="BatchGetDiscoverableRegistryRecord,\
//! ListDiscoverableRegistryRecords,SearchDiscoverableRegistryRecords")`), cited per row.
//! The harness (`seeded_router`, `post`, `list_path`, `encode_label`) is the one
//! `tests/all/discovery_read_path.rs` already drives this service with.

use serde_json::{json, Value};

use crate::discovery_read_path::{
    encode_label, list_path, post, record_ids, seeded_router, EMPTY_REGISTRY, REGISTRY,
};

/// The fixed text every discovery read states for a registry it does not hold; the
/// identifier the request carried is the only variable part.
const NOT_FOUND_PREFIX: &str = "Registry not found: ";

/// `com.amazonaws.agentregistry#RegistryIdentifier`'s `@pattern` as the service
/// advertises it: the modelled expression
/// `^(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}$`
/// without the pair of whole-value anchors the model wraps it in.
const REGISTRY_IDENTIFIER_PATTERN: &str =
    r"(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/)?[a-zA-Z0-9]{12,16}";

/// `com.amazonaws.agentregistry#RecordIdentifier`'s `@pattern` (the element shape of
/// `RegistryRecordsEntry$recordIds`), advertised the same way.
const RECORD_IDENTIFIER_PATTERN: &str = r"(arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/[a-zA-Z0-9]{12,16}/record/)?[a-zA-Z0-9]{12}";

/// A record id that satisfies `RecordIdentifier` (12 alphanumerics), so a refusal of a
/// batch request is about the registry it names and nothing else.
const WELL_FORMED_RECORD_ID: &str = "rec000000001";

/// The `message` member of a refusal body.
fn stated(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The error code a refusal body carries (`__type` on restJson1).
fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The AWS envelope over one constraint clause.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// Every discovery read, as the request that reaches it for `registry`: the operation's
/// path and the body that names that registry and nothing else wrong.
fn reads_of(registry: &str) -> [(&'static str, String, Value); 3] {
    [
        (
            "ListDiscoverableRegistryRecords",
            list_path(registry),
            json!({}),
        ),
        (
            "SearchDiscoverableRegistryRecords",
            "/discoverable-records-search".to_string(),
            json!({ "searchQuery": "", "registryIds": [registry], "maxResults": 10 }),
        ),
        (
            "BatchGetDiscoverableRegistryRecord",
            "/discoverable-records-batch".to_string(),
            json!({ "entries": [
                { "registryId": registry, "recordIds": [WELL_FORMED_RECORD_ID] },
            ] }),
        ),
    ]
}

/// A registry identifier the model admits but this store does not hold is refused by
/// every one of the three reads, with the one sentence and the identifier as supplied.
#[tokio::test]
async fn a_registry_this_service_does_not_hold_is_refused_by_every_read() {
    let router = seeded_router();
    // Well-formed `RegistryIdentifier` values (bare id of 12, 14 and 16 characters —
    // the pattern's `[a-zA-Z0-9]{12,16}` run and both of its bounds — and the ARN form
    // in two partitions) that name no seeded registry.
    let absent: [(&str, &str); 5] = [
        ("bare id at the pattern's minimum length", "zzz999999999"),
        ("bare id of mixed case", "Zz9MissingXyz1"),
        ("bare id at the pattern's maximum length", "ZZ99missing00000"),
        (
            "registry ARN",
            "arn:aws:agent-registry:us-east-1:000000000000:registry/zzz999999999",
        ),
        (
            "registry ARN in another partition",
            "arn:aws-us-gov:agent-registry:us-gov-west-1:000000000000:registry/zzz999999999",
        ),
    ];

    for (form, identifier) in absent {
        for (operation, path, body) in reads_of(identifier) {
            let case = format!("{operation} with a {form}");
            let (status, response) = post(&router, &path, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status}: {response}"
            );
            let code = error_code(&response);
            assert!(
                code.contains("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {code:?}: {response}"
            );
            let expected = format!("{NOT_FOUND_PREFIX}{identifier}");
            let actual = stated(&response);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// The contrast: a registry this store DOES hold is served, whatever it holds — the
/// 404 above is about the registry's existence and not about the records a request
/// asked for. `BatchGetDiscoverableRegistryRecord` reports those per entry, as its
/// `errors` list is modelled to.
#[tokio::test]
async fn a_registry_that_exists_is_served_and_missing_records_are_reported_per_entry() {
    let router = seeded_router();
    // (case, the registry an entry names, the ids it asks for, records returned, errors)
    let cases: [(&str, &str, Value, usize, usize); 4] = [
        (
            "a seeded registry, every id resolvable",
            REGISTRY,
            json!([WELL_FORMED_RECORD_ID]),
            1,
            0,
        ),
        (
            "a seeded registry, one id it does not expose",
            REGISTRY,
            json!([WELL_FORMED_RECORD_ID, "zzz999999999"]),
            1,
            1,
        ),
        (
            // Exists, holds no record whose status is discoverable.
            "a registry holding no discoverable record",
            EMPTY_REGISTRY,
            json!([WELL_FORMED_RECORD_ID]),
            0,
            1,
        ),
        (
            "the same registry named by its ARN",
            "arn:aws:agent-registry:us-east-1:000000000000:registry/abc123def456",
            json!([WELL_FORMED_RECORD_ID]),
            1,
            0,
        ),
    ];

    for (case, registry, record_id_list, records, errors) in cases {
        let (status, response) = post(
            &router,
            "/discoverable-records-batch",
            &json!({ "entries": [{ "registryId": registry, "recordIds": record_id_list }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {response}"
        );
        assert_eq!(
            record_ids(case, &response).len(),
            records,
            "case {case:?}: expected {records} records, got {response}"
        );
        assert_eq!(
            response["errors"].as_array().map(Vec::len),
            Some(errors),
            "case {case:?}: expected {errors} per-entry errors, got {response}"
        );
    }

    // And every read answers 200 for a registry that exists but holds nothing
    // discoverable, so the refusal above cannot be read as "this registry is empty".
    for (operation, path, body) in reads_of(EMPTY_REGISTRY) {
        let (status, response) = post(&router, &path, &body).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: an existing registry must be served, got {status}: {response}"
        );
    }
}

/// A collection member outside its modelled `@length` names the member and the bound
/// and quotes nothing: a list has no scalar rendering, so neither its element count nor
/// its elements reach the sentence.
#[tokio::test]
async fn a_collection_outside_its_length_quotes_nothing_of_the_collection() {
    let router = seeded_router();
    let eleven_filters: Vec<Value> = (0..11)
        .map(|_| json!({ "name": "recordType", "values": ["MCP"] }))
        .collect();
    // (case, path, body, the clause the envelope carries). Every bound is the model's:
    // `entries` and `registryIds` are `@length{min:1,max:1}`, `filters` is
    // `@length{max:10}`.
    let cases: [(&str, &str, Value, String); 5] = [
        (
            "entries over its maximum of one",
            "/discoverable-records-batch",
            json!({ "entries": [
                { "registryId": REGISTRY, "recordIds": [WELL_FORMED_RECORD_ID] },
                { "registryId": REGISTRY, "recordIds": [WELL_FORMED_RECORD_ID] },
            ] }),
            "Value at 'entries' failed to satisfy constraint: Member must have length less \
             than or equal to 1"
                .to_string(),
        ),
        (
            "entries below its minimum of one",
            "/discoverable-records-batch",
            json!({ "entries": [] }),
            "Value at 'entries' failed to satisfy constraint: Member must have length \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            "registryIds over its maximum of one",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "maxResults": 10,
                    "registryIds": [REGISTRY, "bulkreg00001"] }),
            "Value at 'registryIds' failed to satisfy constraint: Member must have length \
             less than or equal to 1"
                .to_string(),
        ),
        (
            "registryIds below its minimum of one",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "maxResults": 10, "registryIds": [] }),
            "Value at 'registryIds' failed to satisfy constraint: Member must have length \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            // A member the issue this suite closes did not cite: the same rule, a
            // different bound, on the other operation.
            "filters over its maximum of ten",
            &list_path(REGISTRY),
            json!({ "filters": eleven_filters }),
            "Value at 'filters' failed to satisfy constraint: Member must have length less \
             than or equal to 10"
                .to_string(),
        ),
    ];

    for (case, path, body, clause) in cases {
        let (status, response) = post(&router, path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let code = error_code(&response);
        assert!(
            code.contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {code:?}: {response}"
        );
        let expected = one_error(&clause);
        let actual = stated(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The same rule where the collection is nested inside a list element: the clause still
/// states the member and the bound and quotes no count.
/// `RegistryRecordsEntry$recordIds` is `@length{min:1,max:100}`.
#[tokio::test]
async fn a_nested_collection_outside_its_length_quotes_nothing_either() {
    let router = seeded_router();
    let hundred_and_one: Vec<String> = (1..=101).map(|n| format!("rec{n:09}")).collect();
    for (case, record_id_list, bound) in [
        ("recordIds over its maximum", json!(hundred_and_one), "less than or equal to 100"),
        ("recordIds below its minimum", json!([]), "greater than or equal to 1"),
    ] {
        let (status, response) = post(
            &router,
            "/discoverable-records-batch",
            &json!({ "entries": [{ "registryId": REGISTRY, "recordIds": record_id_list }] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let actual = stated(&response);
        assert!(
            actual.starts_with("1 validation error detected: Value at '")
                && actual.ends_with(&format!(
                    "failed to satisfy constraint: Member must have length {bound}"
                )),
            "case {case:?}: expected the member and the bound with no value, got {actual:?}"
        );
        assert!(
            !actual.contains("items") && !actual.contains("Value '"),
            "case {case:?}: nothing of the collection may be quoted, got {actual:?}"
        );
    }
}

/// A value outside its member's `@pattern` is refused with the expression that member
/// admits, quoted without the whole-value anchors the model wraps it in, and with no
/// value quoted: the requirement is the expression, not the one value that missed it.
#[tokio::test]
async fn a_pattern_violation_states_the_unanchored_expression_and_quotes_no_value() {
    let router = seeded_router();
    // Values outside `RegistryIdentifier`'s pattern, reached through the `@httpLabel`
    // `registryId` of ListDiscoverableRegistryRecords.
    let bad_ids: [(&str, &str); 5] = [
        ("a space and a punctuation character", "bad id!"),
        ("shorter than the pattern's run", "short"),
        ("longer than the pattern's run", "abcdefghijklmnopqr"),
        ("a punctuation character inside the run", "abc123def456!"),
        (
            "an ARN whose registry segment is malformed",
            "arn:aws:agent-registry:us-east-1:000000000000:registry/nope",
        ),
    ];
    let expected = one_error(&format!(
        "Value at 'registryId' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {REGISTRY_IDENTIFIER_PATTERN}"
    ));

    for (case, bad_id) in bad_ids {
        let path = format!(
            "/registries/{}/discoverable-records-list",
            encode_label(bad_id)
        );
        let (status, response) = post(&router, &path, &json!({})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let actual = stated(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(bad_id)
                // The anchors the model wraps the expression in would show as the
                // first and last characters of the quoted expression; the `^` inside
                // `[^:]` is part of the expression itself.
                && !actual.contains("pattern: ^")
                && !actual.ends_with('$'),
            "case {case:?}: neither the refused value nor an anchor may reach the sentence, \
             got {actual:?}"
        );
    }
}

/// The same class on members the issue this suite closes did not cite: a constraint
/// broken by the ELEMENTS of a collection is stated at that MEMBER, carrying the whole
/// set of requirements its elements have — the element shape's declared set, not just
/// the constraint one value happened to break, since the set is a property of the model
/// rather than of the request.
#[tokio::test]
async fn a_constraint_its_elements_broke_is_stated_at_the_collection_with_its_whole_set() {
    let router = seeded_router();
    // (case, path, body, the member the clause names, the set its elements must satisfy).
    // Every element shape is the model's: `RegistryIdList` over `RegistryIdentifier`
    // (@length 1..2048, @pattern), `RegistryRecordIdList` over `RecordIdentifier`
    // (@length 1..2048, @pattern) and `DiscoverableFilterValues` over `FilterValue`
    // (@length 1..255, no pattern).
    let cases: [(&str, &str, Value, &str, String); 3] = [
        (
            "a registryIds element outside RegistryIdentifier's pattern",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "maxResults": 10, "registryIds": ["bad id!"] }),
            "registryIds",
            format!(
                "Member must have length less than or equal to 2048, Member must have \
                 length greater than or equal to 1, Member must satisfy regular expression \
                 pattern: {REGISTRY_IDENTIFIER_PATTERN}"
            ),
        ),
        (
            // Declared inside `RegistryRecordsEntry`, so the clause is stated at the
            // nested member path the walk reported the element at.
            "a recordIds element outside RecordIdentifier's pattern",
            "/discoverable-records-batch",
            json!({ "entries": [{ "registryId": REGISTRY, "recordIds": ["bad id!"] }] }),
            "entries.1.member.recordIds",
            format!(
                "Member must have length less than or equal to 2048, Member must have \
                 length greater than or equal to 1, Member must satisfy regular expression \
                 pattern: {RECORD_IDENTIFIER_PATTERN}"
            ),
        ),
        (
            // An element shape that declares a `@length` and no pattern, so the set is
            // the two length requirements alone.
            "a filter values element shorter than FilterValue's minimum",
            "",
            json!({ "filters": [{ "name": "recordType", "values": [""] }] }),
            "filters.1.member.values",
            "Member must have length less than or equal to 255, Member must have length \
             greater than or equal to 1"
                .to_string(),
        ),
    ];

    for (case, path, body, member, set) in cases {
        // An empty path row addresses the list operation, whose path carries a label.
        let path = if path.is_empty() {
            list_path(REGISTRY)
        } else {
            path.to_string()
        };
        let (status, response) = post(&router, &path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let expected = one_error(&format!(
            "Value at '{member}' failed to satisfy constraint: Member must satisfy \
             constraint: [{set}]"
        ));
        let actual = stated(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The rule the two tests above are instances of, over every OTHER constraint class
/// this model declares: a clause names the member and the requirement and quotes no
/// value, whatever the request carried — so the refusal a caller reads back is the same
/// for every value outside the same bound.
#[tokio::test]
async fn no_constraint_clause_quotes_the_value_the_request_carried() {
    let router = seeded_router();
    let search = "/discoverable-records-search";
    let list = list_path(REGISTRY);
    let over_long_query = "q".repeat(300);
    // (case, path, body, the clause the envelope carries). `maxResults` is
    // `@range{min:1,max:100}` on the list and `@range{min:1,max:20}` on the search;
    // `searchQuery` is `@length{min:0,max:256}` and `@sensitive`; `entries` is
    // `@required`.
    let cases: [(&str, &str, Value, &str); 6] = [
        (
            "list maxResults one past its maximum",
            &list,
            json!({ "maxResults": 101 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             less than or equal to 100",
        ),
        (
            "list maxResults one below its minimum",
            &list,
            json!({ "maxResults": 0 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1",
        ),
        (
            "search maxResults one past its maximum",
            search,
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 21 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             less than or equal to 20",
        ),
        (
            "search maxResults below its minimum",
            search,
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": -1 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1",
        ),
        (
            "searchQuery past its maximum length",
            search,
            json!({ "searchQuery": over_long_query, "registryIds": [REGISTRY],
                    "maxResults": 10 }),
            "Value at 'searchQuery' failed to satisfy constraint: Member must have length \
             less than or equal to 256",
        ),
        (
            "entries absent altogether",
            "/discoverable-records-batch",
            json!({}),
            "Value at 'entries' failed to satisfy constraint: Member must not be null",
        ),
    ];

    for (case, path, body, clause) in cases {
        let (status, response) = post(&router, path, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let expected = one_error(clause);
        let actual = stated(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains("Value '"),
            "case {case:?}: no clause may quote a value, got {actual:?}"
        );
    }
}

/// Each bound at its published edge is SERVED, so the refusals above are about values
/// outside the model's bounds and not about the members themselves.
#[tokio::test]
async fn a_value_at_a_published_bound_is_served() {
    let router = seeded_router();
    let cases: [(&str, String, Value); 4] = [
        (
            "list maxResults at its minimum",
            list_path(REGISTRY),
            json!({ "maxResults": 1 }),
        ),
        (
            "list maxResults at its maximum",
            list_path(REGISTRY),
            json!({ "maxResults": 100 }),
        ),
        (
            "search maxResults at its maximum",
            "/discoverable-records-search".to_string(),
            json!({ "searchQuery": "", "registryIds": [REGISTRY], "maxResults": 20 }),
        ),
        (
            "recordIds at its minimum of one",
            "/discoverable-records-batch".to_string(),
            json!({ "entries": [
                { "registryId": REGISTRY, "recordIds": [WELL_FORMED_RECORD_ID] },
            ] }),
        ),
    ];

    for (case, path, body) in cases {
        let (status, response) = post(&router, &path, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {response}"
        );
    }
}
