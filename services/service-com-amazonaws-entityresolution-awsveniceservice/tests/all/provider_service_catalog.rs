// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Region's provider-service catalog:
//! `ListProviderServices` and `GetProviderService`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `entityresolution` signing name, through the
//! harness the sibling `schema_mapping_workflow_lifecycle` suite owns — so what is
//! asserted is what an AWS SDK client would see.
//!
//! The rule under test: both operations read the SAME persisted catalog of provider
//! services, so every entry a page reports reads back through `GetProviderService`
//! field-for-field; `providerName` selects exactly that provider's entries;
//! `maxResults` bounds a page inside the modelled range and anything outside it is
//! refused; a `nextToken` that is a catalog key resumes at that key and one this
//! service never minted is refused; the `providerServiceArn` both operations report is
//! `arn:aws:entityresolution:<region>::providerservice/<providerName>/<providerServiceName>`
//! with EVERY segment — the Region included — taken from the catalog entry, so the
//! same entry reads identically whatever region a request is signed for and an ARN
//! whose Region segment is not the entry's resolves to nothing; and every miss is a
//! 404 carrying this operation's one fixed not-found text. Neither operation changes
//! what a later caller sees.
//!
//! Each table spans what the rule generalises over: every entry of the catalog (both
//! `ServiceType`s, both `anonymizedOutput` values, entries whose ARNs name different
//! Regions, a provider with several services and providers with one), several signed
//! Regions, the modelled `maxResults` minimum, maximum and one past each, every
//! catalog key as a resume point, and each way a token or an identifier can fail to
//! name an entry. The recorded list-then-get round trip is one row of these.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::schema_mapping_workflow_lifecycle::{
    bool_member, list_member, string_member, Emulator, Reply, REGION,
};

/// The whole modelled `ServiceType` enum
/// (`smithy_query(query="shape",shape="ServiceType")`). Every catalog entry carries
/// one of these, and the catalog as a whole covers both.
const SERVICE_TYPES: [&str; 2] = ["ASSIGNMENT", "ID_MAPPING"];

/// The modelled `maxResults` range for `ListProviderServices`: `range 15..25`
/// (`smithy_query(query="operation",operation="ListProviderServices")`).
const MAX_RESULTS_MIN: i64 = 15;
const MAX_RESULTS_MAX: i64 = 25;

/// `GetProviderService`'s `providerServiceName` `@httpLabel` is a `ProviderServiceArn`,
/// and a non-greedy label is ONE path segment, so the `/`s inside the ARN ride
/// percent-encoded — which is what an AWS SDK sends.
fn label(arn: &str) -> String {
    arn.replace('/', "%2F")
}

/// The ARN the rule says both operations report for a catalog entry whose own Region
/// is `region`.
fn expected_arn(region: &str, provider_name: &str, provider_service_name: &str) -> String {
    format!(
        "arn:aws:entityresolution:{region}::providerservice/{provider_name}/{provider_service_name}"
    )
}

/// The Region segment of a `providerServiceArn`.
fn arn_region(case: &str, arn: &str) -> String {
    arn.split(':')
        .nth(3)
        .unwrap_or_else(|| panic!("case {case:?}: {arn:?} is not a provider-service ARN"))
        .to_string()
}

/// The fixed `ResourceNotFoundException$message` `GetProviderService` answers for a
/// provider/service pair that is in no catalog: AWS's own text for the miss, which
/// echoes nothing of the request.
const NOT_FOUND_MESSAGE: &str = "PartnerComponent Not Found";

async fn list(emulator: &Emulator, region: &str, query: &str) -> Reply {
    emulator
        .call_in_region(region, "GET", &format!("/providerservices{query}"), None)
        .await
}

async fn get(emulator: &Emulator, region: &str, provider_name: &str, arn: &str) -> Reply {
    emulator
        .call_in_region(
            region,
            "GET",
            &format!("/providerservices/{provider_name}/{}", label(arn)),
            None,
        )
        .await
}

/// The summaries of a page the rule says succeeds.
fn summaries(case: &str, reply: &Reply) -> Vec<Value> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListProviderServices must answer 200, got {reply:?}"
    );
    list_member(&reply.body, "providerServiceSummaries").clone()
}

/// The catalog keys of a page, in the order the page returned them.
fn keys(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            format!(
                "{}/{}",
                string_member(entry, "providerName"),
                string_member(entry, "providerServiceName")
            )
        })
        .collect()
}

/// One member of an object, which the rule says is present.
fn member<'a>(case: &str, record: &'a Value, name: &str) -> &'a Value {
    record
        .get(name)
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| panic!("case {case:?}: member `{name}` is missing from {record:?}"))
}

/// The strings of a JSON list of strings.
fn strings(case: &str, list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {list:?}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string, got {value:?}"))
                .to_string()
        })
        .collect()
}

/// Every entry `ListProviderServices` reports reads back through
/// `GetProviderService`, and each detail record is internally consistent.
#[tokio::test]
async fn every_listed_provider_service_round_trips_through_get() {
    let emulator = Emulator::new();
    let listed = summaries("the whole catalog", &list(&emulator, REGION, "").await);
    assert!(
        listed.len() >= 2,
        "the Region's provider-service catalog must not be empty, got {listed:?}"
    );

    for summary in &listed {
        let provider_name = string_member(summary, "providerName");
        let provider_service_name = string_member(summary, "providerServiceName");
        let case = format!("{provider_name}/{provider_service_name}");
        let arn = string_member(summary, "providerServiceArn");
        // Every segment of the ARN is the entry's own: the provider and service
        // tokens, and a Region segment of the modelled `[a-z]{2}-[a-z]{1,10}-[0-9]`
        // form that the entry carries rather than the caller.
        let entry_region = arn_region(&case, &arn);
        assert_eq!(
            arn,
            expected_arn(&entry_region, &provider_name, &provider_service_name),
            "case {case:?}: the summary's providerServiceArn must be the entry's own ARN, got \
             {arn:?}"
        );
        let region_parts: Vec<&str> = entry_region.split('-').collect();
        assert!(
            region_parts.len() == 3
                && region_parts[0].len() == 2
                && (1..=10).contains(&region_parts[1].len())
                && region_parts[2].len() == 1
                && entry_region
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'),
            "case {case:?}: the ARN's Region segment must match the modelled \
             [a-z]{{2}}-[a-z]{{1,10}}-[0-9], got {entry_region:?}"
        );
        let service_type = string_member(summary, "providerServiceType");
        assert!(
            SERVICE_TYPES.contains(&service_type.as_str()),
            "case {case:?}: providerServiceType must be a modelled ServiceType, got \
             {service_type:?}"
        );
        let display_name = string_member(summary, "providerServiceDisplayName");
        assert!(
            !display_name.is_empty() && display_name.chars().count() <= 255,
            "case {case:?}: providerServiceDisplayName must be a non-empty name within the \
             modelled length of 255, got {display_name:?}"
        );

        let read = get(&emulator, REGION, &provider_name, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetProviderService must answer the entry the list named, got {read:?}"
        );
        for shared in [
            "providerName",
            "providerServiceName",
            "providerServiceDisplayName",
            "providerServiceType",
            "providerServiceArn",
        ] {
            assert_eq!(
                string_member(&read.body, shared),
                string_member(summary, shared),
                "case {case:?}: the list and the read must agree on {shared}, got {:?}",
                read.body
            );
        }

        // `providerEndpointConfiguration` is modelled @required: a union whose only
        // member carries the Data Exchange identifiers of the provider service. Each
        // of the four is a Data Exchange id, and the four are distinct.
        let marketplace = member(
            &case,
            member(&case, &read.body, "providerEndpointConfiguration"),
            "marketplaceConfiguration",
        );
        let mut ids = BTreeSet::new();
        for id_member in ["dataSetId", "revisionId", "assetId", "listingId"] {
            let id = string_member(marketplace, id_member);
            assert!(
                id.len() == 32
                    && id
                        .chars()
                        .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
                "case {case:?}: {id_member} must be a 32-character lowercase hex Data Exchange \
                 id, got {id:?}"
            );
            ids.insert(id);
        }
        assert_eq!(
            ids.len(),
            4,
            "case {case:?}: the four Data Exchange ids must be distinct, got {marketplace:?}"
        );

        // `anonymizedOutput` and `providerEntityOutputDefinition` are both modelled
        // @required.
        let _ = bool_member(&read.body, "anonymizedOutput");
        let output_definition = member(&case, &read.body, "providerEntityOutputDefinition");
        assert!(
            output_definition
                .as_object()
                .is_some_and(|members| !members.is_empty()),
            "case {case:?}: providerEntityOutputDefinition must be a non-empty document, got \
             {output_definition:?}"
        );

        // The input schema names the same fields twice over — once as
        // `providerSchemaAttributes`, once as the single `schemas` entry — so the two
        // cannot describe different inputs.
        let component_schema = member(&case, &read.body, "providerComponentSchema");
        let attribute_names: Vec<String> = list_member(component_schema, "providerSchemaAttributes")
            .iter()
            .map(|attribute| string_member(attribute, "fieldName"))
            .collect();
        assert!(
            !attribute_names.is_empty(),
            "case {case:?}: the entry must declare its input schema attributes, got \
             {component_schema:?}"
        );
        let schemas = list_member(component_schema, "schemas");
        assert_eq!(
            schemas.len(),
            1,
            "case {case:?}: the entry declares exactly one schema, got {schemas:?}"
        );
        assert_eq!(
            strings(&case, &schemas[0]),
            attribute_names,
            "case {case:?}: `schemas` must name the same fields as `providerSchemaAttributes`, \
             got {schemas:?}"
        );
        for attribute in list_member(component_schema, "providerSchemaAttributes") {
            // A provider service's published schema names each field by the
            // `SchemaAttributeType` it accepts there, which is what the single
            // `schemas` entry lists.
            assert_eq!(
                string_member(attribute, "fieldName"),
                string_member(attribute, "type"),
                "case {case:?}: a provider schema attribute's fieldName is the attribute type, \
                 got {attribute:?}"
            );
        }

        // `providerEntityOutputDefinition` declares the one `entity` the provider
        // answers per input record, and that entity's own identifier property.
        assert_eq!(
            strings(&case, member(&case, output_definition, "required")),
            vec!["entity".to_string()],
            "case {case:?}: the entity output definition requires `entity`, got \
             {output_definition:?}"
        );
        let entity = member(
            &case,
            member(&case, output_definition, "properties"),
            "entity",
        );
        let entity_properties = member(&case, entity, "properties")
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: `entity` declares properties, got {entity:?}"));
        assert_eq!(
            entity_properties.len(),
            1,
            "case {case:?}: the entity carries the provider's own identifier, got \
             {entity_properties:?}"
        );

        // `providerConfigurationDefinition` is the schema of the `providerConfiguration`
        // document a workflow hands this provider, so everything it REQUIRES is one of
        // the properties it declares.
        if let Some(definition) = read
            .body
            .get("providerConfigurationDefinition")
            .filter(|value| !value.is_null())
        {
            let properties = member(&case, definition, "properties")
                .as_object()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the configuration definition declares properties")
                });
            for required in strings(&case, member(&case, definition, "required")) {
                assert!(
                    properties.contains_key(&required),
                    "case {case:?}: the configuration definition requires {required:?}, which it \
                     must also declare, got {definition:?}"
                );
            }
        }

        // The accounts-and-actions configuration is present only for a provider that
        // needs an intermediate bucket, and exactly those entries declare the job
        // option that names one.
        let intermediate = read
            .body
            .get("providerIntermediateDataAccessConfiguration")
            .filter(|value| !value.is_null());
        let job_configuration = read
            .body
            .get("providerJobConfiguration")
            .filter(|value| !value.is_null());
        assert_eq!(
            intermediate.is_some(),
            job_configuration.is_some(),
            "case {case:?}: the intermediate-data configuration and the job configuration that \
             names the intermediate location go together, got {:?}",
            read.body
        );
        if let (Some(intermediate), Some(job_configuration)) = (intermediate, job_configuration) {
            for action in strings(&case, member(&case, intermediate, "requiredBucketActions")) {
                assert!(
                    action.starts_with("s3:"),
                    "case {case:?}: a required bucket action is an S3 action, got {action:?}"
                );
            }
            for account_id in strings(&case, member(&case, intermediate, "awsAccountIds")) {
                assert!(
                    account_id.len() == 12 && account_id.chars().all(|ch| ch.is_ascii_digit()),
                    "case {case:?}: an awsAccountIds entry is a 12-digit account id, got \
                     {account_id:?}"
                );
            }
            let job_properties = member(&case, job_configuration, "properties");
            let intermediate_source = member(&case, job_properties, "intermediateSourceConfiguration");
            assert!(
                member(&case, intermediate_source, "properties")
                    .get("intermediateS3Path")
                    .is_some(),
                "case {case:?}: the job configuration names the intermediate S3 path, got \
                 {job_configuration:?}"
            );
        }

        // `providerIdNameSpaceConfiguration` is "the provider configuration required
        // for different ID namespace types", and an ID namespace is the source or the
        // target of an ID-mapping workflow, so exactly the `ID_MAPPING` entries carry
        // one.
        let has_namespace_configuration = read
            .body
            .get("providerIdNameSpaceConfiguration")
            .is_some_and(|value| !value.is_null());
        assert_eq!(
            has_namespace_configuration,
            service_type == "ID_MAPPING",
            "case {case:?}: an ID_MAPPING entry carries an ID-namespace configuration and an \
             ASSIGNMENT entry does not, got {:?}",
            read.body
        );
    }

    // The catalog spans the modelled enum, so neither service type is unreachable.
    let service_types: BTreeSet<String> = listed
        .iter()
        .map(|entry| string_member(entry, "providerServiceType"))
        .collect();
    for service_type in SERVICE_TYPES {
        assert!(
            service_types.contains(service_type),
            "the catalog must offer a {service_type} provider service, got {service_types:?}"
        );
    }

    // Both `anonymizedOutput` values occur, so neither branch of that member is
    // unreachable either.
    let mut anonymized = BTreeSet::new();
    for summary in &listed {
        let read = get(
            &emulator,
            REGION,
            &string_member(summary, "providerName"),
            &string_member(summary, "providerServiceArn"),
        )
        .await;
        anonymized.insert(bool_member(&read.body, "anonymizedOutput"));
    }
    assert_eq!(
        anonymized,
        BTreeSet::from([false, true]),
        "the catalog must offer both an anonymized and a non-anonymized provider service, got \
         {anonymized:?}"
    );
}

/// `providerName` selects exactly that provider's entries, and nothing else's.
#[tokio::test]
async fn provider_name_filters_the_catalog() {
    let emulator = Emulator::new();
    let listed = summaries("the whole catalog", &list(&emulator, REGION, "").await);

    // The expectation is read off the unfiltered page, so this spans every provider
    // the catalog holds — the one with several services as well as those with one.
    let mut by_provider: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, summary) in keys(&listed).into_iter().zip(&listed) {
        by_provider
            .entry(string_member(summary, "providerName"))
            .or_default()
            .push(key);
    }
    assert!(
        by_provider.len() >= 2,
        "the catalog must hold more than one provider for a filter to mean anything, got \
         {by_provider:?}"
    );
    assert!(
        by_provider.values().any(|entries| entries.len() > 1),
        "the catalog must hold a provider with several services, got {by_provider:?}"
    );

    for (provider_name, expected) in &by_provider {
        let filtered = summaries(
            provider_name,
            &list(&emulator, REGION, &format!("?providerName={provider_name}")).await,
        );
        assert_eq!(
            &keys(&filtered),
            expected,
            "case {provider_name:?}: the filter must select exactly that provider's services, \
             got {filtered:?}"
        );
    }

    // A provider nobody offers selects nothing — an empty page, not every entry, and
    // not a refusal. `providerName` is matched exactly, so a provider token that
    // differs only in case or is a prefix of a real one selects nothing either.
    for unknown in ["NoSuchProvider", "LiveRamp", "live"] {
        let reply = list(&emulator, REGION, &format!("?providerName={unknown}")).await;
        let filtered = summaries(unknown, &reply);
        assert!(
            filtered.is_empty(),
            "case {unknown:?}: an unknown providerName must select nothing, got {filtered:?}"
        );
        assert!(
            reply.body.get("nextToken").is_none_or(Value::is_null),
            "case {unknown:?}: an empty page must carry no nextToken, got {:?}",
            reply.body
        );
    }
}

/// `maxResults` bounds a page inside the modelled range, and a value outside it is
/// refused rather than clamped.
#[tokio::test]
async fn max_results_is_honoured_inside_the_modelled_range() {
    let emulator = Emulator::new();
    let catalog_size = summaries("the whole catalog", &list(&emulator, REGION, "").await).len();

    // The modelled minimum, the modelled maximum and one in between.
    for bound in [MAX_RESULTS_MIN, (MAX_RESULTS_MIN + MAX_RESULTS_MAX) / 2, MAX_RESULTS_MAX] {
        let case = format!("maxResults={bound}");
        let reply = list(&emulator, REGION, &format!("?maxResults={bound}")).await;
        let page = summaries(&case, &reply);
        let expected = catalog_size.min(usize::try_from(bound).expect("a positive bound"));
        assert_eq!(
            page.len(),
            expected,
            "case {case:?}: a page holds min(bound, catalog size) entries, got {page:?}"
        );
        // The modelled minimum page already exceeds the catalog, so a page is never
        // partial and never carries a continuation token.
        assert_eq!(
            reply.body.get("nextToken").is_some_and(|t| !t.is_null()),
            page.len() < catalog_size,
            "case {case:?}: a nextToken appears exactly when entries were left out, got {:?}",
            reply.body
        );
    }

    // One past each published bound is refused by the modelled range.
    for bound in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let case = format!("maxResults={bound}");
        let reply = list(&emulator, REGION, &format!("?maxResults={bound}")).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a maxResults outside the modelled range 15..25 must be refused, got \
             {reply:?}"
        );
    }
}

/// A `nextToken` that is a catalog key resumes the scan at that key; one this service
/// never minted is refused with a 400 `ValidationException`.
#[tokio::test]
async fn next_token_resumes_at_a_catalog_key_and_refuses_anything_else() {
    let emulator = Emulator::new();
    let listed = summaries("the whole catalog", &list(&emulator, REGION, "").await);
    let all_keys = keys(&listed);

    // Every key is a resume point, and resuming at one returns exactly the suffix of
    // the catalog from it.
    for (index, token) in all_keys.iter().enumerate() {
        let reply = list(&emulator, REGION, &format!("?nextToken={token}")).await;
        let resumed = summaries(token, &reply);
        assert_eq!(
            keys(&resumed),
            all_keys[index..],
            "case {token:?}: resuming must return the catalog from that key onwards, got \
             {resumed:?}"
        );
    }

    // A token this service never issued is refused, including ones the modelled
    // `NextToken` pattern (`^[a-zA-Z_0-9-=+/]*$`) itself admits.
    for token in [
        "liveramp",
        "liveramp/assignment/EXTRA",
        "/assignment",
        "liveramp/",
        "AAAAFQ==",
    ] {
        let reply = list(&emulator, REGION, &format!("?nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: a token this service never issued must be refused, got {reply:?}"
        );
        assert!(
            reply
                .error_code
                .as_deref()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {token:?}: the refusal is a ValidationException, got {reply:?}"
        );
    }
}

/// A `providerServiceArn` is catalog data: every segment of it, its Region included,
/// comes from the entry, so a page and a read answer the same whatever region the
/// request was signed for, and an ARN whose Region segment is not the entry's names no
/// entry at all.
#[tokio::test]
async fn provider_service_arns_come_from_the_entry_not_the_request() {
    let emulator = Emulator::new();
    // Several signing Regions, each matching the modelled `[a-z]{2}-[a-z]{1,10}-[0-9]`
    // ARN segment: the default one and two no catalog entry is published in.
    let signed_regions = [REGION, "us-west-2", "eu-central-1"];

    let baseline = summaries("the whole catalog", &list(&emulator, REGION, "").await);
    // The catalog really does span Regions, so "the ARN's Region is the entry's" is
    // not satisfiable by any one region.
    let entry_regions: BTreeSet<String> = baseline
        .iter()
        .map(|summary| {
            arn_region(
                "the whole catalog",
                &string_member(summary, "providerServiceArn"),
            )
        })
        .collect();
    assert!(
        entry_regions.len() >= 2,
        "the catalog must publish entries in more than one Region, got {entry_regions:?}"
    );

    for region in signed_regions {
        let listed = summaries(region, &list(&emulator, region, "").await);
        assert_eq!(
            listed, baseline,
            "case {region:?}: the catalog a page reports must not depend on the region the \
             request was signed for, got {listed:?}"
        );
        for summary in &listed {
            let provider_name = string_member(summary, "providerName");
            let provider_service_name = string_member(summary, "providerServiceName");
            let case = format!("signed {region}, {provider_name}/{provider_service_name}");
            let arn = string_member(summary, "providerServiceArn");
            let entry_region = arn_region(&case, &arn);

            let read = get(&emulator, region, &provider_name, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the ARN a page reported must resolve however the read is \
                 signed, got {read:?}"
            );
            assert_eq!(
                string_member(&read.body, "providerServiceArn"),
                arn,
                "case {case:?}: the read must report the entry's own ARN, got {:?}",
                read.body
            );

            // The same provider and service under any other Region segment — the
            // signed region among them — is not an entry of the catalog.
            for other in signed_regions
                .iter()
                .copied()
                .chain(entry_regions.iter().map(String::as_str))
                .filter(|other| *other != entry_region)
            {
                let foreign = expected_arn(other, &provider_name, &provider_service_name);
                let refused = get(&emulator, region, &provider_name, &foreign).await;
                assert_eq!(
                    refused.status, 404,
                    "case {case:?}: an ARN naming {other} rather than the entry's {entry_region} \
                     must not resolve, got {refused:?}"
                );
            }
        }
    }
}

/// Everything that is not an entry of the catalog is a 404
/// `ResourceNotFoundException` carrying this operation's one fixed not-found text.
#[tokio::test]
async fn an_unknown_provider_service_is_not_found() {
    let emulator = Emulator::new();
    let listed = summaries("the whole catalog", &list(&emulator, REGION, "").await);
    let first = &listed[0];
    let last = listed
        .last()
        .expect("the catalog holds at least one provider service");
    // The first entry's own Region, so a case that is meant to differ from it in one
    // way only differs in that one way.
    let first_region = arn_region("the first entry", &string_member(first, "providerServiceArn"));

    let cases = [
        (
            "an ARN of one provider under another provider's name",
            string_member(last, "providerName"),
            string_member(first, "providerServiceArn"),
        ),
        (
            "a service name no provider offers",
            string_member(first, "providerName"),
            expected_arn(
                &first_region,
                &string_member(first, "providerName"),
                "NO_SUCH_SERVICE",
            ),
        ),
        (
            "a provider name nobody offers",
            "NoSuchProvider".to_string(),
            expected_arn(&first_region, "NoSuchProvider", "assignment"),
        ),
        (
            "a provider name that differs only in case",
            string_member(first, "providerName").to_uppercase(),
            expected_arn(
                &first_region,
                &string_member(first, "providerName").to_uppercase(),
                &string_member(first, "providerServiceName"),
            ),
        ),
        (
            "a service name that differs only in case",
            string_member(first, "providerName"),
            expected_arn(
                &first_region,
                &string_member(first, "providerName"),
                &string_member(first, "providerServiceName").to_uppercase(),
            ),
        ),
    ];

    for (case, provider_name, arn) in cases {
        // A case that is only a 404 because the two halves disagree must still be a
        // pair the other half of the catalog really holds.
        let reply = get(&emulator, REGION, &provider_name, &arn).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: an identifier that names no catalog entry must be refused, got \
             {reply:?}"
        );
        assert!(
            reply
                .error_code
                .as_deref()
                .is_some_and(|code| code.contains("ResourceNotFoundException")),
            "case {case:?}: the refusal is a ResourceNotFoundException, got {reply:?}"
        );
        // One fixed text for every miss: it names neither the provider nor the service
        // that was asked for, so it cannot vary with the request.
        assert_eq!(
            string_member(&reply.body, "message"),
            NOT_FOUND_MESSAGE,
            "case {case:?}: the refusal carries this operation's fixed not-found message, got \
             {:?}",
            reply.body
        );
    }
}

/// Both operations are reads: calling either twice answers identically, and neither
/// changes what the other sees.
#[tokio::test]
async fn reading_the_catalog_changes_nothing() {
    let emulator = Emulator::new();

    let first_page = list(&emulator, REGION, "").await;
    let listed = summaries("the whole catalog", &first_page);
    let reads: Vec<Value> = {
        let mut reads = Vec::new();
        for summary in &listed {
            let read = get(
                &emulator,
                REGION,
                &string_member(summary, "providerName"),
                &string_member(summary, "providerServiceArn"),
            )
            .await;
            reads.push(read.body);
        }
        reads
    };

    let second_page = list(&emulator, REGION, "").await;
    assert_eq!(
        second_page.body, first_page.body,
        "case {:?}: a second ListProviderServices must answer identically, got {:?}",
        "repeated list", second_page.body
    );
    for (summary, expected) in listed.iter().zip(&reads) {
        let case = format!(
            "{}/{}",
            string_member(summary, "providerName"),
            string_member(summary, "providerServiceName")
        );
        let read = get(
            &emulator,
            REGION,
            &string_member(summary, "providerName"),
            &string_member(summary, "providerServiceArn"),
        )
        .await;
        assert_eq!(
            &read.body, expected,
            "case {case:?}: a second GetProviderService must answer identically, got {:?}",
            read.body
        );
    }
}
