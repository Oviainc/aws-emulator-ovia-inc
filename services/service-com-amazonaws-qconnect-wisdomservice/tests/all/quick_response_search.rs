// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SearchQuickResponses`, driven over HTTP through the same router
//! the emulator serves with.
//!
//! The rule under test, asserted over TABLES of inputs rather than the one recorded
//! request: a search of a knowledge base reports the quick responses that knowledge base
//! HOLDS which the `searchExpression` selects — a record matching at least one `queries`
//! entry (an expression carrying none selects the whole knowledge base) and every
//! `filters` entry — projected out of the stored record, ordered by `orderOnField` and
//! bounded by `maxResults` with a `nextToken` that continues the same order.
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * the queried field — each of the four the model documents as queryable — and the
//!   filtered field: every one of the twelve the model documents as filterable, including
//!   the list-valued ones and one no record carries a value for;
//! * the query operator (both modelled values), the filter operator (both modelled
//!   values), `allowFuzziness` (absent, false, true) and `includeNoExistence` (absent,
//!   true);
//! * how many values one entry carries (one, several) and how many records the expression
//!   selects (none, one, several, all);
//! * the ordering (none, ascending, descending) and the page size, walked to exhaustion
//!   through the continuation token, which is scoped to the ordering it was minted under.
//!
//! Every hit is compared against what `GetQuickResponse` answers for the same id, so a
//! result that is not computed from persisted state cannot pass. The harness — the
//! router, the knowledge-base create, the quick-response create and the reply reader — is
//! the one `quick_response.rs` already drives this surface with.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::quick_response::{
    path, Emulator, Reply, ABSENT_UUID, CONTENT_TYPE_MARKDOWN, CONTENT_TYPE_PLAIN, STATUS_CREATED,
};

/// One quick response the suite seeds its knowledge base with. The fields are the ones
/// the search can query, filter and order on, so one table of records serves every row
/// below.
#[derive(Debug, Clone)]
struct Seed {
    name: &'static str,
    content: &'static str,
    content_type: &'static str,
    description: Option<&'static str>,
    shortcut_key: Option<&'static str>,
    /// `None` exercises a create that names no `isActive` at all.
    is_active: Option<bool>,
    channels: &'static [&'static str],
    language: Option<&'static str>,
    grouping_criteria: Option<&'static str>,
    grouping_values: &'static [&'static str],
}

impl Seed {
    fn create_body(&self) -> Value {
        let mut body = json!({
            "name": self.name,
            "content": { "content": self.content },
            "contentType": self.content_type,
        });
        if let Some(description) = self.description {
            body["description"] = json!(description);
        }
        if let Some(shortcut_key) = self.shortcut_key {
            body["shortcutKey"] = json!(shortcut_key);
        }
        if let Some(is_active) = self.is_active {
            body["isActive"] = json!(is_active);
        }
        if !self.channels.is_empty() {
            body["channels"] = json!(self.channels);
        }
        if let Some(language) = self.language {
            body["language"] = json!(language);
        }
        if self.grouping_criteria.is_some() || !self.grouping_values.is_empty() {
            let mut grouping = json!({});
            if let Some(criteria) = self.grouping_criteria {
                grouping["criteria"] = json!(criteria);
            }
            if !self.grouping_values.is_empty() {
                grouping["values"] = json!(self.grouping_values);
            }
            body["groupingConfiguration"] = grouping;
        }
        body
    }
}

/// The seeded knowledge base: records that differ in every searchable field, so a row can
/// select one, several or none of them. Two names differ only in letter case, which is
/// what the fuzziness rows turn on.
const SEEDS: [Seed; 5] = [
    Seed {
        name: "order-shipped",
        content: "Your order has shipped!",
        content_type: CONTENT_TYPE_PLAIN,
        description: Some("shipping update"),
        shortcut_key: Some("/ship"),
        is_active: None,
        channels: &["Chat"],
        language: Some("en_US"),
        grouping_criteria: Some("RoutingProfileArn"),
        grouping_values: &["sales"],
    },
    Seed {
        name: "order-cancelled",
        content: "Your order was cancelled.",
        content_type: CONTENT_TYPE_PLAIN,
        description: Some("cancellation notice"),
        shortcut_key: Some("/cancel"),
        is_active: Some(false),
        channels: &["Chat", "Email"],
        language: Some("en_US"),
        grouping_criteria: None,
        grouping_values: &[],
    },
    Seed {
        name: "GREETING-formal",
        content: "Good day, how may I help?",
        content_type: CONTENT_TYPE_MARKDOWN,
        description: None,
        shortcut_key: None,
        is_active: Some(true),
        channels: &[],
        language: Some("fr_FR"),
        grouping_criteria: None,
        grouping_values: &[],
    },
    Seed {
        name: "refund-policy",
        content: "Refunds take 3-5 business days.",
        content_type: CONTENT_TYPE_PLAIN,
        description: Some("policy text"),
        shortcut_key: Some("/refund"),
        is_active: Some(true),
        channels: &["Voice"],
        language: Some("de_DE"),
        grouping_criteria: Some("AgentArn"),
        grouping_values: &["tier-2", "tier-3"],
    },
    Seed {
        name: "greeting-casual",
        content: "Hey! What's up?",
        content_type: CONTENT_TYPE_PLAIN,
        description: None,
        shortcut_key: None,
        is_active: None,
        channels: &[],
        language: Some("en_US"),
        grouping_criteria: None,
        grouping_values: &[],
    },
];

/// The seeded knowledge base, with the `quickResponseId` each seed's create minted, in
/// the order [`SEEDS`] lists them.
struct Seeded {
    emulator: Emulator,
    knowledge_base_id: String,
    ids: Vec<String>,
}

impl Seeded {
    async fn new(knowledge_base_name: &str) -> Self {
        let emulator = Emulator::new();
        let knowledge_base_id = emulator.knowledge_base_id(knowledge_base_name).await;
        let mut ids = Vec::new();
        for seed in &SEEDS {
            let created = emulator
                .create_quick_response(&knowledge_base_id, seed.create_body())
                .await;
            assert_eq!(
                created.status,
                200,
                "case {:?}: the seed quick response must be created, got HTTP {} with {}",
                seed.name,
                created.status,
                created.body
            );
            let id = created
                .string("/quickResponse/quickResponseId")
                .unwrap_or_else(|| panic!("case {:?}: the create answered no id", seed.name));
            ids.push(id);
        }
        Self {
            emulator,
            knowledge_base_id,
            ids,
        }
    }

    /// `SearchQuickResponses` on the seeded knowledge base.
    async fn search(&self, expression: Value) -> Reply {
        self.search_page(expression, None, None).await
    }

    /// `SearchQuickResponses` with the paging members a continuation needs.
    async fn search_page(
        &self,
        expression: Value,
        max_results: Option<usize>,
        next_token: Option<&str>,
    ) -> Reply {
        let mut uri = format!(
            "/knowledgeBases/{}/search/quickResponses",
            path(&self.knowledge_base_id)
        );
        let mut query = Vec::new();
        if let Some(max_results) = max_results {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(next_token) = next_token {
            query.push(format!("nextToken={next_token}"));
        }
        if !query.is_empty() {
            uri.push('?');
            uri.push_str(&query.join("&"));
        }
        self.emulator
            .send("POST", &uri, Some(json!({ "searchExpression": expression })))
            .await
    }

    /// The ids the seeds at `indexes` were created under.
    fn expected(&self, indexes: &[usize]) -> BTreeSet<String> {
        indexes.iter().map(|index| self.ids[*index].clone()).collect()
    }
}

/// The `quickResponseId`s one search answered, as a set: the rows below are about WHICH
/// records an expression selects, which is independent of the order they are paged in.
fn hit_ids(case: &str, reply: &Reply) -> BTreeSet<String> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        reply.status, reply.body
    );
    let results = reply
        .field("/results")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: results is required, got {}", reply.body));
    results
        .iter()
        .map(|hit| {
            hit.get("quickResponseId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a hit carries no id, got {hit}"))
                .to_string()
        })
        .collect()
}

/// One query row: the expression's `queries` entry and the seeds it selects.
#[derive(Debug)]
struct QueryCase {
    label: &'static str,
    field: &'static str,
    operator: &'static str,
    values: &'static [&'static str],
    allow_fuzziness: Option<bool>,
    selects: &'static [usize],
}

/// RULE: a `queries` entry selects the quick responses whose named field one of its
/// values matches under its operator; several entries are alternatives, and a record is a
/// hit when at least one selects it.
#[tokio::test]
async fn a_query_selects_the_stored_quick_responses_whose_field_it_matches() {
    let seeded = Seeded::new("search-queries-kb").await;

    // Every field the model documents as queryable
    // (`smithy_query(query="doc",shape="QuickResponseQueryField")`: content, name,
    // description, shortcutKey), both modelled operators, and every `allowFuzziness`
    // setting.
    let cases = [
        QueryCase {
            label: "the whole name, the way a client looks a reply up by it",
            field: "name",
            operator: "CONTAINS",
            values: &["order-shipped"],
            allow_fuzziness: None,
            selects: &[0],
        },
        QueryCase {
            label: "a name fragment two records share",
            field: "name",
            operator: "CONTAINS",
            values: &["order-"],
            allow_fuzziness: None,
            selects: &[0, 1],
        },
        QueryCase {
            label: "two values, which are alternatives",
            field: "name",
            operator: "CONTAINS",
            values: &["refund", "cancelled"],
            allow_fuzziness: None,
            selects: &[1, 3],
        },
        QueryCase {
            label: "a value no record holds",
            field: "name",
            operator: "CONTAINS",
            values: &["no-record-holds-this"],
            allow_fuzziness: None,
            selects: &[],
        },
        QueryCase {
            label: "an exact match is case sensitive without fuzziness",
            field: "name",
            operator: "CONTAINS",
            values: &["greeting"],
            allow_fuzziness: None,
            selects: &[4],
        },
        QueryCase {
            label: "fuzziness off is the same exact match",
            field: "name",
            operator: "CONTAINS",
            values: &["greeting"],
            allow_fuzziness: Some(false),
            selects: &[4],
        },
        QueryCase {
            label: "fuzziness on matches whatever the letter case",
            field: "name",
            operator: "CONTAINS",
            values: &["greeting"],
            allow_fuzziness: Some(true),
            selects: &[2, 4],
        },
        QueryCase {
            label: "the content of one record",
            field: "content",
            operator: "CONTAINS",
            values: &["has shipped"],
            allow_fuzziness: None,
            selects: &[0],
        },
        QueryCase {
            label: "a description only one record carries",
            field: "description",
            operator: "CONTAINS",
            values: &["cancellation"],
            allow_fuzziness: None,
            selects: &[1],
        },
        QueryCase {
            label: "a description no record carries is no hit",
            field: "description",
            operator: "CONTAINS",
            values: &["Hey!"],
            allow_fuzziness: None,
            selects: &[],
        },
        QueryCase {
            label: "a shortcut key",
            field: "shortcutKey",
            operator: "CONTAINS",
            values: &["/refu"],
            allow_fuzziness: None,
            selects: &[3],
        },
        QueryCase {
            label: "the prefix operator on a single term",
            field: "name",
            operator: "CONTAINS_AND_PREFIX",
            values: &["order-ship"],
            allow_fuzziness: None,
            selects: &[0],
        },
        QueryCase {
            label: "the prefix operator: the terms before the last must occur",
            field: "content",
            operator: "CONTAINS_AND_PREFIX",
            values: &["Your order ship"],
            allow_fuzziness: None,
            selects: &[0],
        },
        QueryCase {
            label: "the prefix operator with a term no record's content holds",
            field: "content",
            operator: "CONTAINS_AND_PREFIX",
            values: &["Your refund ship"],
            allow_fuzziness: None,
            selects: &[],
        },
    ];

    for case in &cases {
        let mut query = json!({
            "name": case.field,
            "values": case.values,
            "operator": case.operator,
        });
        if let Some(allow_fuzziness) = case.allow_fuzziness {
            query["allowFuzziness"] = json!(allow_fuzziness);
        }
        let reply = seeded.search(json!({ "queries": [query] })).await;
        let actual = hit_ids(case.label, &reply);
        assert_eq!(
            actual,
            seeded.expected(case.selects),
            "case {:?}: expected the seeds {:?} to be selected, got {actual:?} from {}",
            case.label,
            case.selects,
            reply.body
        );
    }

    // Several entries in one expression are alternatives: the union of what each selects.
    let reply = seeded
        .search(json!({
            "queries": [
                { "name": "name", "values": ["refund"], "operator": "CONTAINS" },
                { "name": "content", "values": ["has shipped"], "operator": "CONTAINS" },
            ]
        }))
        .await;
    let actual = hit_ids("two query entries", &reply);
    assert_eq!(
        actual,
        seeded.expected(&[0, 3]),
        "case \"two query entries\": expected the union of what each selects, got {actual:?} \
         from {}",
        reply.body
    );

    // An expression carrying no query at all selects the whole knowledge base.
    let reply = seeded.search(json!({})).await;
    let actual = hit_ids("an empty expression", &reply);
    assert_eq!(
        actual,
        seeded.expected(&[0, 1, 2, 3, 4]),
        "case \"an empty expression\": expected every seeded quick response, got {actual:?} \
         from {}",
        reply.body
    );
}

/// One filter row: the expression's `filters` entry and the seeds it admits.
#[derive(Debug)]
struct FilterCase {
    label: &'static str,
    field: &'static str,
    operator: &'static str,
    values: &'static [&'static str],
    include_no_existence: Option<bool>,
    selects: &'static [usize],
}

/// RULE: a `filters` entry restricts the selection to the records whose named field one
/// of its values matches under its operator, and `includeNoExistence` is what admits a
/// record carrying no value for that field.
#[tokio::test]
async fn filters_restrict_the_selection_to_the_records_they_admit() {
    let seeded = Seeded::new("search-filters-kb").await;

    // Every field the model documents as filterable
    // (`smithy_query(query="doc",shape="QuickResponseFilterField")`: name, description,
    // shortcutKey, isActive, channels, language, contentType, createdTime,
    // lastModifiedTime, lastModifiedBy, groupingConfiguration.criteria,
    // groupingConfiguration.values) with both modelled operators.
    let cases = [
        FilterCase {
            label: "a name, exactly",
            field: "name",
            operator: "EQUALS",
            values: &["refund-policy"],
            include_no_existence: None,
            selects: &[3],
        },
        FilterCase {
            label: "a name prefix two records share",
            field: "name",
            operator: "PREFIX",
            values: &["order-"],
            include_no_existence: None,
            selects: &[0, 1],
        },
        FilterCase {
            label: "a description, exactly",
            field: "description",
            operator: "EQUALS",
            values: &["policy text"],
            include_no_existence: None,
            selects: &[3],
        },
        FilterCase {
            label: "a description the records without one are admitted beside",
            field: "description",
            operator: "EQUALS",
            values: &["policy text"],
            include_no_existence: Some(true),
            selects: &[2, 3, 4],
        },
        FilterCase {
            label: "a shortcut key prefix",
            field: "shortcutKey",
            operator: "PREFIX",
            values: &["/c"],
            include_no_existence: None,
            selects: &[1],
        },
        FilterCase {
            label: "the records in service, including the one whose create named none",
            field: "isActive",
            operator: "EQUALS",
            values: &["true"],
            include_no_existence: None,
            selects: &[0, 2, 3, 4],
        },
        FilterCase {
            label: "the record a create deactivated",
            field: "isActive",
            operator: "EQUALS",
            values: &["false"],
            include_no_existence: None,
            selects: &[1],
        },
        FilterCase {
            label: "one channel of a list-valued field",
            field: "channels",
            operator: "EQUALS",
            values: &["Email"],
            include_no_existence: None,
            selects: &[1],
        },
        FilterCase {
            label: "a channel two records share, with the channel-less ones admitted",
            field: "channels",
            operator: "EQUALS",
            values: &["Chat"],
            include_no_existence: Some(true),
            selects: &[0, 1, 2, 4],
        },
        FilterCase {
            label: "a language, exactly",
            field: "language",
            operator: "EQUALS",
            values: &["en_US"],
            include_no_existence: None,
            selects: &[0, 1, 4],
        },
        FilterCase {
            label: "a language prefix",
            field: "language",
            operator: "PREFIX",
            values: &["de"],
            include_no_existence: None,
            selects: &[3],
        },
        FilterCase {
            label: "the richtext media type",
            field: "contentType",
            operator: "EQUALS",
            values: &[CONTENT_TYPE_MARKDOWN],
            include_no_existence: None,
            selects: &[2],
        },
        FilterCase {
            label: "the plain-text media type",
            field: "contentType",
            operator: "EQUALS",
            values: &[CONTENT_TYPE_PLAIN],
            include_no_existence: None,
            selects: &[0, 1, 3, 4],
        },
        FilterCase {
            label: "a grouping criterion only one record carries",
            field: "groupingConfiguration.criteria",
            operator: "EQUALS",
            values: &["AgentArn"],
            include_no_existence: None,
            selects: &[3],
        },
        FilterCase {
            label: "one value of a list-valued grouping configuration",
            field: "groupingConfiguration.values",
            operator: "EQUALS",
            values: &["tier-3"],
            include_no_existence: None,
            selects: &[3],
        },
        FilterCase {
            label: "a grouping value no record carries",
            field: "groupingConfiguration.values",
            operator: "PREFIX",
            values: &["tier-9"],
            include_no_existence: None,
            selects: &[],
        },
        FilterCase {
            // No record this emulator holds names the identity that last wrote it, so the
            // field exists on none of them: only `includeNoExistence` admits one.
            label: "a field no record carries a value for",
            field: "lastModifiedBy",
            operator: "EQUALS",
            values: &["arn:aws:iam::123456789012:role/agent"],
            include_no_existence: None,
            selects: &[],
        },
        FilterCase {
            label: "a field no record carries a value for, absence admitted",
            field: "lastModifiedBy",
            operator: "EQUALS",
            values: &["arn:aws:iam::123456789012:role/agent"],
            include_no_existence: Some(true),
            selects: &[0, 1, 2, 3, 4],
        },
    ];

    for case in &cases {
        let mut filter = json!({
            "name": case.field,
            "values": case.values,
            "operator": case.operator,
        });
        if let Some(include_no_existence) = case.include_no_existence {
            filter["includeNoExistence"] = json!(include_no_existence);
        }
        let reply = seeded.search(json!({ "filters": [filter] })).await;
        let actual = hit_ids(case.label, &reply);
        assert_eq!(
            actual,
            seeded.expected(case.selects),
            "case {:?}: expected the seeds {:?} to be admitted, got {actual:?} from {}",
            case.label,
            case.selects,
            reply.body
        );
    }

    // The two timestamp fields every record carries: a prefix of the epoch second this
    // run happens in admits them all, and a prefix no epoch second of this run starts
    // with admits none. The value is taken from the test's own clock, never from a
    // recording.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock at or after the epoch")
        .as_secs()
        .to_string();
    let shared_prefix = &now[..now.len() - 4];
    for field in ["createdTime", "lastModifiedTime"] {
        let case = format!("{field} in the epoch second this run happens in");
        let reply = seeded
            .search(json!({
                "filters": [{ "name": field, "values": [shared_prefix], "operator": "PREFIX" }]
            }))
            .await;
        let actual = hit_ids(&case, &reply);
        assert_eq!(
            actual,
            seeded.expected(&[0, 1, 2, 3, 4]),
            "case {case:?}: expected every seeded quick response, got {actual:?} from {}",
            reply.body
        );

        let case = format!("{field} outside this run");
        let reply = seeded
            .search(json!({
                "filters": [{ "name": field, "values": ["0"], "operator": "PREFIX" }]
            }))
            .await;
        let actual = hit_ids(&case, &reply);
        assert!(
            actual.is_empty(),
            "case {case:?}: expected no hit, got {actual:?} from {}",
            reply.body
        );
    }

    // Filters and queries hold TOGETHER: the query selects, the filter restricts.
    let reply = seeded
        .search(json!({
            "queries": [{ "name": "name", "values": ["order-"], "operator": "CONTAINS" }],
            "filters": [{ "name": "isActive", "values": ["false"], "operator": "EQUALS" }],
        }))
        .await;
    let actual = hit_ids("a query and a filter together", &reply);
    assert_eq!(
        actual,
        seeded.expected(&[1]),
        "case \"a query and a filter together\": expected only the record both admit, got \
         {actual:?} from {}",
        reply.body
    );

    // Two filters are a conjunction: a record has to satisfy both.
    let reply = seeded
        .search(json!({
            "filters": [
                { "name": "language", "values": ["en_US"], "operator": "EQUALS" },
                { "name": "channels", "values": ["Chat"], "operator": "EQUALS" },
            ]
        }))
        .await;
    let actual = hit_ids("two filters", &reply);
    assert_eq!(
        actual,
        seeded.expected(&[0, 1]),
        "case \"two filters\": expected only the records both admit, got {actual:?} from {}",
        reply.body
    );
}

/// RULE: a hit reports the quick response out of the STORED record, so every member it
/// carries is the one `GetQuickResponse` answers for the same id.
#[tokio::test]
async fn a_hit_reports_the_same_record_a_read_of_it_answers() {
    let seeded = Seeded::new("search-projection-kb").await;

    for (index, seed) in SEEDS.iter().enumerate() {
        let reply = seeded
            .search(json!({
                "queries": [{ "name": "name", "values": [seed.name], "operator": "CONTAINS" }]
            }))
            .await;
        let hits = reply
            .field("/results")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            hits.len(),
            1,
            "case {:?}: expected exactly the record named, got {}",
            seed.name,
            reply.body
        );
        let hit = &hits[0];
        let read = seeded
            .emulator
            .get_quick_response(&seeded.knowledge_base_id, &seeded.ids[index])
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the record must read back, got HTTP {} with {}",
            seed.name, read.status, read.body
        );

        // The members both shapes declare must agree, value for value.
        for member in [
            "quickResponseId",
            "quickResponseArn",
            "knowledgeBaseId",
            "knowledgeBaseArn",
            "name",
            "contentType",
            "status",
            "contents",
            "createdTime",
            "lastModifiedTime",
            "description",
            "shortcutKey",
            "channels",
            "language",
            "groupingConfiguration",
        ] {
            let expected = read.field(&format!("/quickResponse/{member}")).cloned();
            let actual = hit.get(member).filter(|value| !value.is_null()).cloned();
            assert_eq!(
                actual, expected,
                "case {:?}: the hit's {member} must be the stored one, got {actual:?} against \
                 {expected:?}",
                seed.name
            );
        }
        assert_eq!(
            hit.get("status").and_then(Value::as_str),
            Some(STATUS_CREATED),
            "case {:?}: expected the stored status, got {hit}",
            seed.name
        );
        // `isActive` is required on a hit while the create may name none: a record nothing
        // deactivated is reported in service.
        assert_eq!(
            hit.get("isActive").and_then(Value::as_bool),
            Some(seed.is_active.unwrap_or(true)),
            "case {:?}: expected the stored isActive, got {hit}",
            seed.name
        );
    }
}

/// RULE: the hits are ordered by `orderOnField` — ascending unless its `order` says
/// `DESC` — and `maxResults` bounds the page, with `nextToken` continuing the SAME order
/// until every selected record has been served exactly once.
#[tokio::test]
async fn the_page_is_ordered_and_continued_by_the_token_it_hands_out() {
    let seeded = Seeded::new("search-paging-kb").await;

    // Ordering on a field: every record, in the order its values sort in.
    let by_name: Vec<&str> = {
        let mut names: Vec<&str> = SEEDS.iter().map(|seed| seed.name).collect();
        names.sort_unstable();
        names
    };
    let descending_by_name: Vec<&str> = by_name.iter().rev().copied().collect();
    let order_cases: [(&str, Value, &Vec<&str>); 3] = [
        (
            "ordered on name, no direction named",
            json!({ "orderOnField": { "name": "name" } }),
            &by_name,
        ),
        (
            "ordered on name, ascending",
            json!({ "orderOnField": { "name": "name", "order": "ASC" } }),
            &by_name,
        ),
        (
            "ordered on name, descending",
            json!({ "orderOnField": { "name": "name", "order": "DESC" } }),
            &descending_by_name,
        ),
    ];
    for (case, expression, expected) in order_cases {
        let reply = seeded.search(expression).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        let names: Vec<String> = reply
            .field("/results")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: results is required, got {}", reply.body))
            .iter()
            .map(|hit| {
                hit.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names,
            expected.iter().map(|name| name.to_string()).collect::<Vec<String>>(),
            "case {case:?}: expected the hits in that order, got {names:?}"
        );
    }

    // Paging: every page size from one to past the selection, walked to exhaustion. Each
    // walk must serve every selected record exactly once, in one order.
    for page_size in [1usize, 2, 3, 5, 6] {
        for (case, expression) in [
            ("no order field", json!({})),
            (
                "ordered on name descending",
                json!({ "orderOnField": { "name": "name", "order": "DESC" } }),
            ),
        ] {
            let case = format!("{case}, pages of {page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let reply = seeded
                    .search_page(expression.clone(), Some(page_size), token.as_deref())
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with {}",
                    reply.status, reply.body
                );
                let page = reply
                    .field("/results")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: a page may not exceed maxResults, got {} entries",
                    page.len()
                );
                for hit in &page {
                    seen.push(
                        hit.get("quickResponseId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    );
                }
                token = reply.string("/nextToken");
                pages += 1;
                assert!(
                    pages <= SEEDS.len() + 2,
                    "case {case:?}: the walk must terminate, got {pages} pages for {} records",
                    SEEDS.len()
                );
                if token.is_none() {
                    break;
                }
            }
            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case:?}: no record may be served twice, got {seen:?}"
            );
            assert_eq!(
                unique,
                seeded.expected(&[0, 1, 2, 3, 4]),
                "case {case:?}: the walk must serve every selected record, got {seen:?}"
            );
        }
    }

    // The last page of a selection carries no continuation.
    let reply = seeded.search_page(json!({}), Some(100), None).await;
    assert_eq!(
        reply.string("/nextToken"),
        None,
        "case \"a selection that fits one page\": expected no nextToken, got {}",
        reply.body
    );
}

/// RULE: a search of a knowledge base this account does not hold is the modelled
/// `ResourceNotFoundException` (404); an expression naming a field outside the catalog it
/// belongs to, a token this service did not mint, and a token replayed against an
/// ordering it was not minted for are each the modelled `ValidationException` (400).
#[tokio::test]
async fn a_search_this_service_cannot_serve_is_refused_with_its_modelled_error() {
    let seeded = Seeded::new("search-refusal-kb").await;

    let absent = seeded
        .emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{ABSENT_UUID}/search/quickResponses"),
            Some(json!({ "searchExpression": {} })),
        )
        .await;
    assert_eq!(
        (absent.status, absent.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a knowledge base this account does not hold\": expected HTTP 404 \
         ResourceNotFoundException, got {} {:?} with {}",
        absent.status,
        absent.error_type,
        absent.body
    );

    let refusals = [
        (
            "a queries entry naming a field this service does not query by",
            "searchExpression.queries",
            json!({ "queries": [{ "name": "isActive", "values": ["true"], "operator": "CONTAINS" }] }),
        ),
        (
            "a queries entry naming a field of no shape at all",
            "searchExpression.queries",
            json!({ "queries": [{ "name": "nickname", "values": ["x"], "operator": "CONTAINS" }] }),
        ),
        (
            "a filters entry naming a field this service does not filter by",
            "searchExpression.filters",
            json!({ "filters": [{ "name": "content", "values": ["x"], "operator": "EQUALS" }] }),
        ),
        (
            "an orderOnField naming a field outside the catalog",
            "searchExpression.orderOnField",
            json!({ "orderOnField": { "name": "content" } }),
        ),
    ];
    for (case, member, expression) in refusals {
        let reply = seeded.search(expression).await;
        assert_eq!(
            (reply.status, reply.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {} {:?} with {}",
            reply.status,
            reply.error_type,
            reply.body
        );
        // The refusal has to say WHICH member was wrong, so the caller can fix the
        // request from the message alone.
        let message = reply.string("/message").unwrap_or_default();
        assert!(
            message.contains(member),
            "case {case:?}: the refusal must name the member {member}, got {message:?}"
        );
    }

    let bad_token = seeded.search_page(json!({}), None, Some("not-a-token")).await;
    assert_eq!(
        (bad_token.status, bad_token.error_type.as_str()),
        (400, "ValidationException"),
        "case \"a token this service did not mint\": expected HTTP 400 ValidationException, got \
         {} {:?} with {}",
        bad_token.status,
        bad_token.error_type,
        bad_token.body
    );

    // A token names a position in ONE order, so one minted for another ordering names no
    // position in this search.
    let first_page = seeded.search_page(json!({}), Some(1), None).await;
    let token = first_page
        .string("/nextToken")
        .expect("the first page of five records carries a continuation");
    for (case, expression) in [
        (
            "replayed against an ordering the token was not minted for",
            json!({ "orderOnField": { "name": "name" } }),
        ),
        (
            "replayed against the other direction",
            json!({ "orderOnField": { "name": "name", "order": "DESC" } }),
        ),
    ] {
        let reply = seeded.search_page(expression, Some(1), Some(&token)).await;
        assert_eq!(
            (reply.status, reply.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {} {:?} with {}",
            reply.status,
            reply.error_type,
            reply.body
        );
    }
    // Replayed against the ordering it WAS minted for, the same token continues the page.
    let continued = seeded.search_page(json!({}), Some(1), Some(&token)).await;
    assert_eq!(
        continued.status, 200,
        "case \"replayed against its own ordering\": expected HTTP 200, got {} with {}",
        continued.status, continued.body
    );
}
