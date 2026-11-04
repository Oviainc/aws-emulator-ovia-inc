// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListIdMappingWorkflows`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness `schema_mapping_workflow_lifecycle` owns — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: a `@paginated` list operation reports EXACTLY the records its
//! own collection persists, in a stable key order, each summary member read from the
//! stored record; `maxResults` bounds a page, the `nextToken` a page carries is the
//! key of the first record it did not return (and the final page carries none), a
//! token this service never issued is refused, and the read leaves the store as it
//! was.
//!
//! Each table row differs along what the rule generalises over rather than repeating
//! one recorded request: the page size over EVERY size the seeded collection admits
//! (plus "no bound" and the modelled maximum), the `workflowName` over the character
//! classes the modelled `EntityName` pattern admits at both length bounds, the number
//! of input sources over the modelled `length` minimum and maximum, and the
//! `idMappingType` over its whole modelled catalog.

use serde_json::{json, Value};

use crate::id_mapping_workflow_lifecycle::{
    provider_service_arn, ID_MAPPING_TYPES, ID_NAMESPACE_TYPES,
};
use crate::schema_mapping_workflow_lifecycle::{
    assert_error_shape, expected_arn, glue_table_arn, list_member, role_arn, string_member,
    timestamp_member, Emulator, Reply,
};

/// `GET /idmappingworkflows` — `ListIdMappingWorkflows`'s modelled HTTP binding
/// (`smithy_query(query="operation",operation="ListIdMappingWorkflows")`).
///
/// `pub(crate)` so the sibling delete suite asserts that a deleted workflow leaves the
/// listing through the SAME path instead of cloning a second listing helper.
pub(crate) async fn list_id_mapping_workflows(emulator: &Emulator, query: &str) -> Reply {
    emulator
        .call("GET", &format!("/idmappingworkflows{query}"), None)
        .await
}

/// `POST /idmappingworkflows` — `CreateIdMappingWorkflow`'s modelled HTTP binding.
async fn create_id_mapping_workflow(emulator: &Emulator, request: &Value) -> Reply {
    emulator
        .call("POST", "/idmappingworkflows", Some(request))
        .await
}

/// `GET /idmappingworkflows/{workflowName}` — `GetIdMappingWorkflow`'s binding, the
/// reader this listing is checked against.
async fn get_id_mapping_workflow(emulator: &Emulator, workflow_name: &str) -> Reply {
    emulator
        .call("GET", &format!("/idmappingworkflows/{workflow_name}"), None)
        .await
}

/// The members `IdMappingWorkflowSummary` models, and the ONLY ones a summary may
/// carry (`smithy_query(query="shape",shape="IdMappingWorkflowSummary")`): all four
/// are @required.
const SUMMARY_MEMBERS: &[&str] = &["workflowName", "workflowArn", "createdAt", "updatedAt"];

/// Members of the full record that `IdMappingWorkflowSummary` does NOT model, so a
/// summary must not carry them.
const NON_SUMMARY_MEMBERS: &[&str] = &[
    "description",
    "inputSourceConfig",
    "outputSourceConfig",
    "idMappingTechniques",
    "incrementalRunConfig",
    "roleArn",
    "tags",
];

/// One value of `IdMappingWorkflowRuleDefinitionType`, from
/// `smithy_query(query="shape",shape="IdMappingRuleBasedProperties")`. The seeds here
/// need one valid value; the sibling lifecycle suite spans that catalog.
const RULE_DEFINITION_TYPE: &str = "SOURCE";

/// A rule-based `IdMappingTechniques`, the cheapest configuration the model accepts.
fn rule_based() -> Value {
    json!({
        "idMappingType": ID_MAPPING_TYPES[1],
        "ruleBasedProperties": {
            "ruleDefinitionType": RULE_DEFINITION_TYPE,
            "attributeMatchingModel": "MANY_TO_MANY",
            "recordMatchingModel": "ONE_SOURCE_TO_ONE_TARGET",
            "rules": [{ "ruleName": "by email", "matchingKeys": ["email"] }],
        },
    })
}

/// A provider-based `IdMappingTechniques`, the other entry of the modelled
/// `IdMappingType` catalog.
fn provider_based() -> Value {
    json!({
        "idMappingType": ID_MAPPING_TYPES[0],
        "providerProperties": {
            "providerServiceArn": provider_service_arn("testprovider", "assignment"),
        },
    })
}

/// `count` distinct `IdMappingWorkflowInputSource` entries, alternating over both ARN
/// forms the modelled `InputSourceARN` admits here and over the whole
/// `IdNamespaceType` catalog.
fn input_sources(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                let namespace_type = ID_NAMESPACE_TYPES[index % ID_NAMESPACE_TYPES.len()];
                if index % 2 == 0 {
                    json!({
                        "inputSourceARN": glue_table_arn(&format!("table_{index}")),
                        "schemaName": format!("schema_{index}"),
                        "type": namespace_type,
                    })
                } else {
                    json!({
                        "inputSourceARN": expected_arn("idnamespace", &format!("ns_{index}")),
                        "type": namespace_type,
                    })
                }
            })
            .collect(),
    )
}

/// One workflow to seed, described by what the rule generalises over.
struct SeedCase {
    case: &'static str,
    workflow_name: String,
    /// `IdMappingWorkflowInputSourceConfig`, modelled `length 1..20`.
    sources: usize,
    id_mapping_techniques: Value,
    /// Optional members are carried by some rows and not others: none of them appears
    /// on a summary either way.
    description: Option<String>,
    role_arn: Option<String>,
}

impl SeedCase {
    fn request(&self) -> Value {
        let mut request = json!({
            "workflowName": self.workflow_name,
            "inputSourceConfig": input_sources(self.sources),
            "idMappingTechniques": self.id_mapping_techniques.clone(),
        });
        let object = request.as_object_mut().expect("a JSON object");
        if let Some(description) = &self.description {
            object.insert("description".to_string(), json!(description));
        }
        if let Some(role_arn) = &self.role_arn {
            object.insert("roleArn".to_string(), json!(role_arn));
        }
        request
    }
}

/// The seed table. Names are deliberately NOT in creation order, so a listing that
/// happened to follow insertion order would not satisfy the assertions below.
fn seed_cases() -> Vec<SeedCase> {
    vec![
        SeedCase {
            case: "a mixed-case, digit, underscore and hyphen name; two input sources",
            workflow_name: "crm_People-1".to_string(),
            sources: 2,
            id_mapping_techniques: provider_based(),
            description: Some("people mapped from the CRM".to_string()),
            role_arn: Some(role_arn()),
        },
        SeedCase {
            case: "the modelled maximum of 20 input sources",
            workflow_name: "widest".to_string(),
            sources: 20,
            id_mapping_techniques: rule_based(),
            description: None,
            role_arn: None,
        },
        SeedCase {
            case: "a one-character name at the modelled length minimum",
            workflow_name: "a".to_string(),
            sources: 1,
            id_mapping_techniques: rule_based(),
            description: Some(String::new()),
            role_arn: Some(String::new()),
        },
        SeedCase {
            case: "an all-digit name",
            workflow_name: "0123456789".to_string(),
            sources: 3,
            id_mapping_techniques: provider_based(),
            description: Some("digits".to_string()),
            role_arn: Some(role_arn()),
        },
        SeedCase {
            case: "a name at the modelled length maximum of 255",
            workflow_name: "W".repeat(255),
            sources: 1,
            id_mapping_techniques: rule_based(),
            description: Some("d".repeat(255)),
            role_arn: None,
        },
        SeedCase {
            case: "an upper-case name, so case ordering is exercised too",
            workflow_name: "DEVICES0".to_string(),
            sources: 1,
            id_mapping_techniques: rule_based(),
            description: None,
            role_arn: Some(role_arn()),
        },
    ]
}

/// Seed every row of the table and answer the names in the order the service must
/// report them (the stable key order).
async fn seed(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    for case in seed_cases() {
        let label = case.case;
        let created = create_id_mapping_workflow(emulator, &case.request()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: seeding the workflow failed, got {created:?}"
        );
        names.push(case.workflow_name);
    }
    names.sort();
    names
}

/// A matching workflow under `name`: a DIFFERENT resource in a different collection,
/// seeded so the assertions below prove the listing reports only its own.
async fn seed_matching_workflow(emulator: &Emulator, name: &str) {
    // The workflow's `inputSourceConfig` references a schema mapping by name, and a
    // matching-workflow write resolves every such reference, so the mapping is created
    // first.
    emulator.seed_schema_mapping(name).await;
    let request = json!({
        "workflowName": name,
        "inputSourceConfig": [
            { "inputSourceARN": glue_table_arn("customers"), "schemaName": name },
        ],
        "outputSourceConfig": [{
            "outputS3Path": "s3://er-output/listing",
            "output": [{ "name": "id" }],
        }],
        "resolutionTechniques": { "resolutionType": "ML_MATCHING" },
        "roleArn": role_arn(),
    });
    let created = emulator.create_matching_workflow(&request).await;
    assert_eq!(
        created.status, 200,
        "seeding matching workflow {name:?} failed, got {created:?}"
    );
}

/// The `workflowName`s of one page, in the order the page listed them.
pub(crate) fn page_names(reply: &Reply) -> Vec<String> {
    list_member(&reply.body, "workflowSummaries")
        .iter()
        .map(|entry| string_member(entry, "workflowName"))
        .collect()
}

fn next_token(reply: &Reply) -> Option<String> {
    reply
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn an_empty_collection_lists_nothing_and_a_matching_workflow_is_not_one_of_its_records() {
    let emulator = Emulator::new();

    let empty = list_id_mapping_workflows(&emulator, "").await;
    assert_eq!(
        empty.status, 200,
        "an empty collection still lists, got {empty:?}"
    );
    assert!(
        page_names(&empty).is_empty(),
        "nothing was created, so the page must be empty, got {:?}",
        empty.body
    );
    assert!(
        next_token(&empty).is_none(),
        "an empty page must carry no nextToken, got {:?}",
        empty.body
    );

    // The two workflow kinds are separate collections with their own ARN types, so a
    // matching workflow is invisible here however it is named.
    for name in ["crm_People-1", "widest", "a"] {
        seed_matching_workflow(&emulator, name).await;
    }
    let still_empty = list_id_mapping_workflows(&emulator, "").await;
    assert_eq!(
        page_names(&still_empty),
        Vec::<String>::new(),
        "a matching workflow is not an ID mapping workflow, got {:?}",
        still_empty.body
    );
}

#[tokio::test]
async fn every_summary_member_is_the_stored_record_and_the_read_changes_nothing() {
    let emulator = Emulator::new();
    let expected = seed(&emulator).await;
    // A matching workflow sharing a name must not change a single summary.
    seed_matching_workflow(&emulator, "widest").await;

    let listed = list_id_mapping_workflows(&emulator, "?maxResults=25").await;
    assert_eq!(listed.status, 200, "expected HTTP 200, got {listed:?}");
    assert_eq!(
        page_names(&listed),
        expected,
        "the listing must report every stored workflow in key order, got {:?}",
        listed.body
    );

    for summary in list_member(&listed.body, "workflowSummaries") {
        let name = string_member(summary, "workflowName");
        let read = get_id_mapping_workflow(&emulator, &name).await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: a listed workflow must read back, got {read:?}"
        );

        for member in SUMMARY_MEMBERS {
            assert!(
                summary.get(member).is_some(),
                "case {name:?}: the @required summary member {member:?} is missing from \
                 {summary:?}"
            );
        }
        for member in NON_SUMMARY_MEMBERS {
            assert!(
                summary.get(member).is_none(),
                "case {name:?}: IdMappingWorkflowSummary models no {member:?}, got {summary:?}"
            );
        }
        assert_eq!(
            string_member(summary, "workflowArn"),
            expected_arn("idmappingworkflow", &name),
            "case {name:?}: the summary must carry the stored idmappingworkflow ARN, got \
             {summary:?}"
        );
        assert_eq!(
            string_member(summary, "workflowArn"),
            string_member(&read.body, "workflowArn"),
            "case {name:?}: the summary and the reader serve one stored record, got \
             {summary:?} and {:?}",
            read.body
        );
        for stamp in ["createdAt", "updatedAt"] {
            assert_eq!(
                timestamp_member(summary, stamp),
                timestamp_member(&read.body, stamp),
                "case {name:?}: the summary's {stamp} must be the stored one, got {summary:?} \
                 and {:?}",
                read.body
            );
        }
        assert_eq!(
            timestamp_member(summary, "createdAt"),
            timestamp_member(summary, "updatedAt"),
            "case {name:?}: a workflow never updated carries equal stamps, got {summary:?}"
        );
    }

    // A read leaves the store exactly as it was: listing twice answers identically,
    // and so does the reader afterwards.
    let again = list_id_mapping_workflows(&emulator, "?maxResults=25").await;
    assert_eq!(
        again.body, listed.body,
        "two consecutive listings must answer identically, got {again:?}"
    );
    for name in &expected {
        let read = get_id_mapping_workflow(&emulator, name).await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: the listing must not have consumed the record, got {read:?}"
        );
    }
}

#[tokio::test]
async fn the_listing_pages_at_max_results_and_resumes_exactly_where_the_token_points() {
    let emulator = Emulator::new();
    let expected = seed(&emulator).await;

    // Every page size the seeded collection admits, plus "no bound at all", the
    // modelled maximum of 25 and a bound larger than the collection. A `maxResults`
    // of 0 is inside the modelled `range ..25`, and the shared `state::page_size`
    // serves the modelled maximum for it rather than an empty page with a token
    // forever.
    let bounds = [
        None,
        Some(0),
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(5),
        Some(6),
        Some(7),
        Some(25),
    ];
    for bound in bounds {
        let label = format!("maxResults={bound:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let query = match (bound, token.as_deref()) {
                (None, None) => String::new(),
                (None, Some(next)) => format!("?nextToken={next}"),
                (Some(size), None) => format!("?maxResults={size}"),
                (Some(size), Some(next)) => format!("?maxResults={size}&nextToken={next}"),
            };
            let reply = list_id_mapping_workflows(&emulator, &query).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200, got {reply:?}"
            );
            let names = page_names(&reply);
            token = next_token(&reply);

            // A token means records were left out, which only a FULL page may do.
            if let (Some(size), Some(_)) = (bound.filter(|size| *size > 0), token.as_deref()) {
                assert_eq!(
                    names.len(),
                    usize::try_from(size).expect("a positive bound"),
                    "case {label:?}: a page followed by a token must be full, got {names:?}"
                );
            }
            assert!(
                !names.is_empty() || token.is_none(),
                "case {label:?}: an empty page must not carry a token, got {reply:?}"
            );
            if let Some(resume_at) = token.as_deref() {
                assert_eq!(
                    Some(resume_at.to_string()),
                    expected.get(walked.len() + names.len()).cloned(),
                    "case {label:?}: the token must name the first key NOT returned, got \
                     {reply:?}"
                );
            }
            walked.extend(names);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {label:?}: paging did not terminate; walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            walked, expected,
            "case {label:?}: the pages concatenated must be the whole collection in key \
             order, got {walked:?}"
        );
        // An absent bound, a zero bound and a bound past the collection all serve it in
        // one page; a positive bound inside it takes ceil(n / size) pages.
        let page_size = match bound {
            None | Some(0) => expected.len(),
            Some(size) => usize::try_from(size)
                .expect("a positive bound")
                .min(expected.len()),
        };
        assert_eq!(
            pages,
            expected.len().div_ceil(page_size),
            "case {label:?}: expected {} pages of at most {page_size}, took {pages}",
            expected.len().div_ceil(page_size)
        );
    }

    // A token resumes at the first key at or after it, which is what makes a page
    // boundary well defined: asked for one record at a time, each token names the next
    // key and resuming from it serves exactly the remaining suffix.
    for (index, name) in expected.iter().enumerate() {
        let resumed = list_id_mapping_workflows(&emulator, &format!("?nextToken={name}")).await;
        assert_eq!(
            resumed.status, 200,
            "case {name:?}: resuming at an issued token must succeed, got {resumed:?}"
        );
        assert_eq!(
            page_names(&resumed),
            expected[index..].to_vec(),
            "case {name:?}: a resumed page must be the suffix from that key, got {:?}",
            resumed.body
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The modelled `NextToken` (`^[a-zA-Z_0-9-=+/]*$`, length 1..1024) is WIDER than
    // the resource names this service issues as tokens (`^[a-zA-Z_0-9-]*$`, 1..255),
    // so each of these satisfies the member's own constraints and still cannot be a
    // token this service minted.
    let unissued = [
        "abc=".to_string(),
        "+".to_string(),
        "a/b".to_string(),
        "ZGVmZ2g=".to_string(),
        "x".repeat(256),
    ];
    for token in &unissued {
        let reply = list_id_mapping_workflows(&emulator, &format!("?nextToken={token}")).await;
        assert_error_shape(token, &reply, 400, "ValidationException");
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    let expected = seed(&emulator).await;

    // `maxResults` is modelled `range ..25`
    // (`smithy_query(query="operation",operation="ListIdMappingWorkflows")`): the
    // published bound is served, one past it is refused.
    let at_bound = list_id_mapping_workflows(&emulator, "?maxResults=25").await;
    assert_eq!(
        at_bound.status, 200,
        "the modelled maximum of 25 must be served, got {at_bound:?}"
    );
    assert_eq!(
        page_names(&at_bound),
        expected,
        "case \"maxResults=25\": the whole collection fits one page, got {:?}",
        at_bound.body
    );

    for over in ["26", "100"] {
        let reply = list_id_mapping_workflows(&emulator, &format!("?maxResults={over}")).await;
        assert_error_shape(over, &reply, 400, "ValidationException");
    }
}
