// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Entity Resolution ID-namespace create/read loop.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is what an AWS SDK client would see, not an internal function's return
//! value.
//!
//! The rule under test: `CreateIdNamespace` PERSISTS a new, uniquely named ID
//! namespace — minting its `idNamespaceArn` in the request's own region — and answers
//! the stored record, so `GetIdNamespace` returns that record field-for-field; a
//! second create of a name that already exists is refused with `ConflictException`
//! (HTTP 400) and leaves the stored record untouched, and a read of a name that was
//! never created is `ResourceNotFoundException` (HTTP 404).
//!
//! Each table row differs along what the rule generalises over: the whole modelled
//! `IdNamespaceType` catalog (`SOURCE`, `TARGET`), the whole modelled `IdMappingType`
//! catalog (`RULE_BASED`, `PROVIDER`) plus an absent `idMappingWorkflowProperties`,
//! the `inputSourceConfig` size over the modelled `length 0..20` minimum, a middle
//! value and the maximum, every optional member over absent and present, and
//! `idNamespaceName` over the character classes the modelled `EntityName` pattern
//! `^[a-zA-Z_0-9-]*$` admits at its `length` minimum of 1 and maximum of 255. The
//! recorded request is ONE row of these.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::id_mapping_workflow_lifecycle::{
    provider_service_arn, ID_MAPPING_TYPES, ID_NAMESPACE_TYPES,
};
use crate::schema_mapping_workflow_lifecycle::{
    account_id, assert_error_shape as assert_error, expected_arn, glue_table_arn, role_arn,
    string_member, timestamp_member, Emulator, REGION,
};

// The two modelled catalogs this suite spans — `IdNamespaceType` (`SOURCE | TARGET`)
// and `IdMappingType` (`PROVIDER | RULE_BASED`) — are declared once, in the sibling
// suite above, and imported here so the two cannot drift.

/// One `CreateIdNamespace` request, described by its members so the expectation is
/// derived from the same data the request is built from.
struct NamespaceCase {
    case: &'static str,
    name: String,
    /// `type` — @required, one of [`ID_NAMESPACE_TYPES`].
    namespace_type: &'static str,
    /// `description` — optional, modelled `length ..255`.
    description: Option<String>,
    /// `roleArn` — optional, modelled `length 32..512`.
    role: Option<String>,
    /// `inputSourceConfig` — optional, modelled `length 0..20`. `None` is "the request
    /// carried none"; `Some(list)` is a list of exactly that many entries.
    input_sources: Option<Vec<Value>>,
    /// `idMappingWorkflowProperties` — optional, modelled `length 1..1`.
    workflow_properties: Option<Value>,
    /// `tags` — optional, modelled `length 0..200`.
    tags: Option<Value>,
}

impl NamespaceCase {
    /// The `CreateIdNamespace` body: only the members this case carries.
    fn request(&self) -> Value {
        let mut request = json!({ "idNamespaceName": self.name, "type": self.namespace_type });
        let object = request
            .as_object_mut()
            .expect("the request body is a JSON object");
        if let Some(description) = &self.description {
            object.insert("description".to_string(), json!(description));
        }
        if let Some(role) = &self.role {
            object.insert("roleArn".to_string(), json!(role));
        }
        if let Some(sources) = &self.input_sources {
            object.insert("inputSourceConfig".to_string(), json!(sources));
        }
        if let Some(properties) = &self.workflow_properties {
            object.insert(
                "idMappingWorkflowProperties".to_string(),
                json!([properties]),
            );
        }
        if let Some(tags) = &self.tags {
            object.insert("tags".to_string(), tags.clone());
        }
        request
    }
}

/// `IdNamespaceInputSource` entries: `inputSourceARN` (a Glue table ARN is one of the
/// forms the modelled pattern admits) plus the optional `schemaName`.
pub(crate) fn input_sources(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            if index % 2 == 0 {
                json!({
                    "inputSourceARN": glue_table_arn(&format!("table_{index}")),
                    "schemaName": format!("schema_{index}"),
                })
            } else {
                // `schemaName` is optional on `IdNamespaceInputSource`.
                json!({ "inputSourceARN": glue_table_arn(&format!("table_{index}")) })
            }
        })
        .collect()
}

/// A `RULE_BASED` `IdNamespaceIdMappingWorkflowProperties`, carrying the optional
/// members of `NamespaceRuleBasedProperties` (`rules` 1..25, `ruleDefinitionTypes`,
/// `attributeMatchingModel` one of `ONE_TO_ONE | MANY_TO_MANY`, `recordMatchingModels`
/// one of `ONE_SOURCE_TO_ONE_TARGET | MANY_SOURCE_TO_ONE_TARGET`).
pub(crate) fn rule_based_properties(
    rules: usize,
    matching_model: &str,
    record_model: &str,
) -> Value {
    let rules: Vec<Value> = (0..rules)
        .map(|index| {
            json!({
                "ruleName": format!("rule {index}"),
                "matchingKeys": [format!("key_{index}")],
            })
        })
        .collect();
    json!({
        "idMappingType": "RULE_BASED",
        "ruleBasedProperties": {
            "rules": rules,
            "ruleDefinitionTypes": ["SOURCE"],
            "attributeMatchingModel": matching_model,
            "recordMatchingModels": [record_model],
        },
    })
}

/// A `PROVIDER` `IdNamespaceIdMappingWorkflowProperties`, whose
/// `NamespaceProviderProperties$providerServiceArn` is @required. The ARN comes from
/// the sibling suite's one `ProviderServiceArn` helper (an EMPTY account segment,
/// `smithy_query(query="shape",shape="NamespaceProviderProperties")`).
pub(crate) fn provider_properties() -> Value {
    json!({
        "idMappingType": "PROVIDER",
        "providerProperties": {
            "providerServiceArn": provider_service_arn("LiveRamp", "IdentityResolution"),
        },
    })
}

/// The `CreateIdNamespace` table.
fn namespace_cases() -> Vec<NamespaceCase> {
    vec![
        NamespaceCase {
            case: "TARGET, no optional member at all, a single-letter name at the \
                   modelled length minimum",
            name: "a".to_string(),
            namespace_type: "TARGET",
            description: None,
            role: None,
            input_sources: None,
            workflow_properties: None,
            tags: None,
        },
        NamespaceCase {
            case: "SOURCE with every optional member, one input source and RULE_BASED \
                   properties",
            name: "Customers".to_string(),
            namespace_type: "SOURCE",
            description: Some("the CRM people namespace".to_string()),
            role: Some(role_arn()),
            input_sources: Some(input_sources(1)),
            workflow_properties: Some(rule_based_properties(
                1,
                "ONE_TO_ONE",
                "ONE_SOURCE_TO_ONE_TARGET",
            )),
            tags: Some(json!({ "team": "data", "env": "test" })),
        },
        NamespaceCase {
            case: "SOURCE with PROVIDER properties and the modelled inputSourceConfig \
                   minimum of zero entries",
            name: "crm_people-1".to_string(),
            namespace_type: "SOURCE",
            description: None,
            role: Some(role_arn()),
            input_sources: Some(input_sources(0)),
            workflow_properties: Some(provider_properties()),
            tags: Some(json!({})),
        },
        NamespaceCase {
            case: "TARGET at the modelled inputSourceConfig maximum of 20 entries and \
                   the modelled description maximum of 255 characters",
            name: "UPPER_lower-123".to_string(),
            namespace_type: "TARGET",
            description: Some("d".repeat(255)),
            role: Some(role_arn()),
            input_sources: Some(input_sources(20)),
            workflow_properties: None,
            tags: Some(json!({ "only": "" })),
        },
        NamespaceCase {
            case: "a digits-only name, three input sources and the many-to-many \
                   RULE_BASED models",
            name: "0".to_string(),
            namespace_type: "SOURCE",
            description: Some(String::new()),
            role: None,
            input_sources: Some(input_sources(3)),
            workflow_properties: Some(rule_based_properties(
                25,
                "MANY_TO_MANY",
                "MANY_SOURCE_TO_ONE_TARGET",
            )),
            tags: None,
        },
        NamespaceCase {
            case: "a name at the modelled length maximum of 255",
            name: "n".repeat(255),
            namespace_type: "TARGET",
            description: None,
            role: Some(role_arn()),
            input_sources: Some(input_sources(2)),
            workflow_properties: Some(provider_properties()),
            tags: None,
        },
    ]
}

/// An optional member of a response body: `None` when the response carries none.
pub(crate) fn optional_member(record: &Value, member: &str) -> Option<Value> {
    record.get(member).cloned()
}

/// Assert `body` is the record `case` asked to be created: every member the request
/// carried comes back unchanged, the ARN is the one the modelled `IdNamespaceArn`
/// pattern describes for this name and region, and the two stamps are equal (nothing
/// has updated the namespace since it was created).
fn assert_is_created_record(case: &NamespaceCase, body: &Value) {
    let label = case.case;
    assert_eq!(
        string_member(body, "idNamespaceName"),
        case.name,
        "case {label:?}: expected the requested idNamespaceName back, got {body:?}"
    );
    assert_eq!(
        string_member(body, "idNamespaceArn"),
        expected_arn("idnamespace", &case.name),
        "case {label:?}: expected the minted idNamespaceArn, got {body:?}"
    );
    assert_eq!(
        string_member(body, "type"),
        case.namespace_type,
        "case {label:?}: expected the requested type back, got {body:?}"
    );
    assert_eq!(
        optional_member(body, "description"),
        case.description.as_ref().map(|text| json!(text)),
        "case {label:?}: expected description {:?}, got {body:?}",
        case.description
    );
    assert_eq!(
        optional_member(body, "roleArn"),
        case.role.as_ref().map(|arn| json!(arn)),
        "case {label:?}: expected roleArn {:?}, got {body:?}",
        case.role
    );
    assert_eq!(
        optional_member(body, "inputSourceConfig"),
        case.input_sources.as_ref().map(|sources| json!(sources)),
        "case {label:?}: expected inputSourceConfig {:?}, got {body:?}",
        case.input_sources
    );
    assert_eq!(
        optional_member(body, "idMappingWorkflowProperties"),
        case.workflow_properties
            .as_ref()
            .map(|properties| json!([properties])),
        "case {label:?}: expected idMappingWorkflowProperties {:?}, got {body:?}",
        case.workflow_properties
    );
    assert_eq!(
        optional_member(body, "tags"),
        case.tags.clone(),
        "case {label:?}: expected tags {:?}, got {body:?}",
        case.tags
    );

    let created_at = timestamp_member(body, "createdAt");
    let updated_at = timestamp_member(body, "updatedAt");
    assert!(
        created_at > 0.0,
        "case {label:?}: createdAt must be a real instant, got {created_at}"
    );
    assert!(
        (created_at - updated_at).abs() < f64::EPSILON,
        "case {label:?}: a namespace that was only just created has not been updated \
         since, so createdAt {created_at} must equal updatedAt {updated_at}"
    );
}

#[tokio::test]
async fn a_created_id_namespace_reads_back_field_for_field() {
    let emulator = Emulator::new();

    for case in namespace_cases() {
        let label = case.case;
        let created = emulator.create_id_namespace(&case.request()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}"
        );
        assert_is_created_record(&case, &created.body);

        // The round trip: the reader answers the record the create persisted, member
        // for member — both outputs model the same members, so the two bodies are the
        // same JSON object.
        let read = emulator.get_id_namespace(&case.name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected HTTP 200 from GetIdNamespace, got {read:?}"
        );
        assert_eq!(
            read.body, created.body,
            "case {label:?}: GetIdNamespace must answer exactly what CreateIdNamespace \
             persisted, got {:?} after {:?}",
            read.body, created.body
        );

        // `GetIdNamespace` is `@readonly`: reading changes nothing a later caller sees.
        let again = emulator.get_id_namespace(&case.name).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: a second read must answer identically, got {:?} after {:?}",
            again.body, read.body
        );
    }
}

#[tokio::test]
async fn every_created_namespace_stays_reachable_and_independent() {
    let emulator = Emulator::new();
    let cases = namespace_cases();
    let mut arns = BTreeSet::new();

    for case in &cases {
        let created = emulator.create_id_namespace(&case.request()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}",
            case.case
        );
        arns.insert(string_member(&created.body, "idNamespaceArn"));
    }

    assert_eq!(
        arns.len(),
        cases.len(),
        "each namespace must get its own ARN, saw {arns:?}"
    );

    // Creating later namespaces leaves every earlier record exactly as it was: each
    // read answers its OWN record, not the last one written.
    for case in &cases {
        let read = emulator.get_id_namespace(&case.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetIdNamespace, got {read:?}",
            case.case
        );
        assert_is_created_record(case, &read.body);
    }
}

#[tokio::test]
async fn a_duplicate_name_conflicts_and_leaves_the_stored_record_untouched() {
    let emulator = Emulator::new();

    for case in namespace_cases() {
        let label = case.case;
        let created = emulator.create_id_namespace(&case.request()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}"
        );

        // "Each ID namespace must have a unique name" — the second create of the same
        // name is refused, whatever else the request says, so the second attempt here
        // also flips `type` and drops every optional member.
        let other_type = ID_NAMESPACE_TYPES
            .iter()
            .find(|candidate| **candidate != case.namespace_type)
            .expect("the modelled catalog has two types");
        let duplicate = emulator
            .create_id_namespace(&json!({
                "idNamespaceName": case.name,
                "type": other_type,
            }))
            .await;
        assert_error(label, &duplicate, 400, "ConflictException");

        // A refused create leaves the store exactly as it was.
        let read = emulator.get_id_namespace(&case.name).await;
        assert_eq!(
            read.body, created.body,
            "case {label:?}: the refused create must not have changed the stored \
             record, got {:?} after {:?}",
            read.body, created.body
        );
    }
}

#[tokio::test]
async fn reading_a_namespace_that_was_never_created_is_not_found() {
    let emulator = Emulator::new();

    for case in namespace_cases() {
        let label = case.case;
        let missing = emulator.get_id_namespace(&case.name).await;
        assert_error(label, &missing, 404, "ResourceNotFoundException");

        // And the same name, once created, is served — so the 404 is decided by what
        // the store holds, not by the name's shape.
        let created = emulator.create_id_namespace(&case.request()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}"
        );
        let found = emulator.get_id_namespace(&case.name).await;
        assert_eq!(
            found.status, 200,
            "case {label:?}: expected HTTP 200 from GetIdNamespace, got {found:?}"
        );
    }
}

#[tokio::test]
async fn the_reader_resolves_a_namespace_by_its_arn_as_well_as_by_its_name() {
    let emulator = Emulator::new();

    for case in namespace_cases() {
        let label = case.case;
        let created = emulator.create_id_namespace(&case.request()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}"
        );
        let arn = string_member(&created.body, "idNamespaceArn");

        // `GetIdNamespace`'s label is an `EntityNameOrIdNamespaceArn`, so the ARN the
        // create minted names the same record the name does. The label is a single
        // path segment, so its `/` travels percent-encoded, exactly as an SDK sends it.
        let by_arn = emulator.get_id_namespace(&arn.replace('/', "%2F")).await;
        assert_eq!(
            by_arn.status, 200,
            "case {label:?}: expected HTTP 200 reading {arn:?}, got {by_arn:?}"
        );
        assert_eq!(
            by_arn.body, created.body,
            "case {label:?}: reading by ARN must answer the same record as reading by \
             name, got {:?} after {:?}",
            by_arn.body, created.body
        );
    }

    // An ARN naming a namespace that does not exist is still a 404, not a hit on
    // whatever the store happens to hold.
    let absent = expected_arn("idnamespace", "never-created").replace('/', "%2F");
    let missing = emulator.get_id_namespace(&absent).await;
    assert_error(
        "an ARN for a namespace that was never created",
        &missing,
        404,
        "ResourceNotFoundException",
    );
}

#[tokio::test]
async fn an_arn_scoped_to_another_partition_region_account_or_service_resolves_to_nothing() {
    let emulator = Emulator::new();
    let name = "scoped";
    let created = emulator
        .create_id_namespace(&json!({ "idNamespaceName": name, "type": "SOURCE" }))
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateIdNamespace, got {created:?}"
    );
    // The local ARN resolves, so the cases below differ from it in exactly one field.
    let local = string_member(&created.body, "idNamespaceArn");
    assert_eq!(
        local,
        expected_arn("idnamespace", name),
        "the create must mint the ARN for this request's own region and account"
    );

    // An `IdNamespaceArn` is `arn:<partition>:entityresolution:<region>:<account>:
    // idnamespace/<name>`: a namespace is scoped to one partition, region and
    // account, so an ARN naming any other names no namespace here. The modelled
    // `EntityNameOrIdNamespaceArn` pattern itself admits only the `entityresolution`
    // service and the `idnamespace/` resource type, so an ARN outside THAT is refused
    // at the boundary with the modelled `ValidationException` instead.
    let cases = [
        (
            "another partition",
            format!(
                "arn:aws-cn:entityresolution:{REGION}:{}:idnamespace/{name}",
                account_id()
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "another region",
            format!(
                "arn:aws:entityresolution:eu-west-1:{}:idnamespace/{name}",
                account_id()
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "another account",
            format!("arn:aws:entityresolution:{REGION}:210987654321:idnamespace/{name}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "another service, which the modelled pattern does not admit",
            format!("arn:aws:glue:{REGION}:{}:idnamespace/{name}", account_id()),
            400,
            "ValidationException",
        ),
        (
            "an ARN naming no resource type, which the modelled pattern does not admit",
            format!("arn:aws:entityresolution:{REGION}:{}:{name}", account_id()),
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, status, code) in cases {
        let refused = emulator.get_id_namespace(&arn.replace('/', "%2F")).await;
        assert_error(case, &refused, status, code);
    }

    // ...while the ARN that differs in none of those fields still resolves.
    let found = emulator.get_id_namespace(&local.replace('/', "%2F")).await;
    assert_eq!(
        found.status, 200,
        "the namespace's own ARN must still resolve, got {found:?}"
    );
}

#[tokio::test]
async fn creates_past_the_enforced_quota_are_refused_and_change_nothing() {
    let emulator = Emulator::new();

    // The bound this emulator enforces for the published, adjustable per-account,
    // per-Region "ID namespaces" quota (`crate::state::MAX_ID_NAMESPACES`), pinned
    // here as a literal so the suite checks the behaviour and not the constant.
    const MAX_ID_NAMESPACES: usize = 100;

    for index in 0..MAX_ID_NAMESPACES {
        let request = json!({
            "idNamespaceName": format!("quota-{index}"),
            "type": ID_NAMESPACE_TYPES[index % ID_NAMESPACE_TYPES.len()],
        });
        let created = emulator.create_id_namespace(&request).await;
        assert_eq!(
            created.status, 200,
            "namespace {index} is inside the quota, so it must be created, got {created:?}"
        );
    }

    let refused = emulator
        .create_id_namespace(&json!({ "idNamespaceName": "one-too-many", "type": "TARGET" }))
        .await;
    assert_error("a create past the quota", &refused, 402, "ExceedsLimitException");
    assert_eq!(
        string_member(&refused.body, "quotaName"),
        "ID namespaces",
        "the refusal must name the quota it enforced, got {:?}",
        refused.body
    );
    assert_eq!(
        refused.body.get("quotaValue").and_then(Value::as_u64),
        Some(MAX_ID_NAMESPACES as u64),
        "the refusal must report the quota value it enforced, got {:?}",
        refused.body
    );

    // Nothing was created for the refused call, and the quota is self-healing: the
    // room a delete-free store has comes back only when an entry is released, so the
    // refused name is still absent.
    let missing = emulator.get_id_namespace("one-too-many").await;
    assert_error(
        "the namespace a quota-refused create would have made",
        &missing,
        404,
        "ResourceNotFoundException",
    );
}

#[tokio::test]
async fn the_whole_modelled_type_catalog_round_trips() {
    let emulator = Emulator::new();
    let mut seen = BTreeSet::new();

    for namespace_type in ID_NAMESPACE_TYPES.iter().copied() {
        for mapping_type in ID_MAPPING_TYPES.iter().copied() {
            let name = format!("catalog-{namespace_type}-{mapping_type}");
            let properties = if mapping_type == "PROVIDER" {
                provider_properties()
            } else {
                rule_based_properties(2, "ONE_TO_ONE", "ONE_SOURCE_TO_ONE_TARGET")
            };
            let case = NamespaceCase {
                case: "the modelled type catalog",
                name: name.clone(),
                namespace_type,
                description: None,
                role: None,
                input_sources: None,
                workflow_properties: Some(properties),
                tags: None,
            };

            let created = emulator.create_id_namespace(&case.request()).await;
            assert_eq!(
                created.status, 200,
                "case {name:?}: expected HTTP 200 from CreateIdNamespace, got {created:?}"
            );
            assert_is_created_record(&case, &created.body);

            let read = emulator.get_id_namespace(&name).await;
            assert_eq!(
                read.body, created.body,
                "case {name:?}: GetIdNamespace must answer what was persisted, got {:?}",
                read.body
            );
            seen.insert((namespace_type, mapping_type));
        }
    }

    assert_eq!(
        seen.len(),
        ID_NAMESPACE_TYPES.len() * ID_MAPPING_TYPES.len(),
        "every (IdNamespaceType, IdMappingType) pair must round trip, saw {seen:?}"
    );
}
