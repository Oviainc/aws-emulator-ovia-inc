// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what this service's 2xx bodies PROJECT.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through the
//! one harness this test crate has (`telemetry_rules`), so what is asserted is what a
//! signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. a resource this service NAMES ITSELF is handed back as an ARN of the resource's
//!    own type carrying the opaque id the service assigned it — never a value the
//!    request supplied, and never the same id twice;
//! 2. the Resource Explorer managed view the telemetry enrichment feature reads is one
//!    view per region, minted the first time that region's feature is started and
//!    reported by every later read of it;
//! 3. a read projects every member the record determines: the reason for the status the
//!    resource is in, and a tag map for a resource whose tag set is empty;
//! 4. a member the request leaves to the service is RECORDED — the telemetry source
//!    types a rule's resource type correlates with — so a read and a listing report the
//!    same list the create settled on;
//! 5. a configuration defect is reported one error per schema problem, with the reason
//!    the violated schema keyword maps to, the schema validator's wording, and a
//!    `FieldMap` naming the property or the path the problem is at.

use regex::Regex;
use serde_json::{json, Map, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::is_minted_resource_id;

use crate::telemetry_rules::{call, emulator, ACCOUNT};

/// The region most requests below sign for.
const HOME_REGION: &str = "us-west-2";

/// A configuration document this emulator runs, used wherever a create needs one.
const PIPELINE_BODY: &str = "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n";

// =============================================================================
// Rule 1 — a service-minted handle
// =============================================================================

/// One resource this service names itself: how a create is asked for, and where the
/// ARN it answers is read from.
struct Minted {
    case: &'static str,
    /// The resource type its ARN carries, which is the resource's own API name.
    resource_type: &'static str,
    create_uri: &'static str,
    request: Value,
    /// A value the REQUEST carried that must not end up standing in for the id.
    submitted: Option<&'static str>,
}

/// The resources this service assigns an id to, and the requests that create them.
///
/// The rows differ along what the rule generalises over: whether the create carries a
/// caller-supplied name at all (the pipeline does, the integration does not), the
/// resource type the ARN carries, and the request's own values.
fn minted_resources() -> Vec<Minted> {
    vec![
        Minted {
            case: "an S3 Table integration, which the caller cannot name",
            resource_type: "s3tableintegration",
            create_uri: "/CreateS3TableIntegration",
            request: json!({
                "Encryption": { "SseAlgorithm": "AES256" },
                "RoleArn": "arn:aws:iam::000000000000:role/telemetry-s3-tables",
            }),
            submitted: None,
        },
        Minted {
            case: "a telemetry pipeline, which the caller names",
            resource_type: "telemetry-pipeline",
            create_uri: "/CreateTelemetryPipeline",
            request: json!({
                "Name": "projection-probe",
                "Configuration": { "Body": PIPELINE_BODY },
            }),
            submitted: Some("projection-probe"),
        },
    ]
}

/// The id inside `arn`, having asserted the ARN is the one this service mints for
/// `resource_type` in `region`: the service's own ARN fields, the resource type, and
/// then the opaque id the service assigned the resource — asked of the service's own
/// predicate, so the test cannot drift from the minting site.
fn minted_id(case: &str, region: &str, resource_type: &str, arn: &str) -> String {
    let prefix = format!("arn:aws:observabilityadmin:{region}:{ACCOUNT}:{resource_type}/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        panic!("case {case:?}: expected an ARN under {prefix}, got {arn:?}");
    };
    assert!(
        is_minted_resource_id(id),
        "case {case:?}: expected an opaque hexadecimal id, got {id:?} in {arn:?}"
    );
    id.to_string()
}

/// The handle a create answers is an ARN of the resource's own type carrying the id
/// the service assigned it: not a value the request supplied, fresh per create, and
/// the handle a later read of the same resource reports.
#[tokio::test]
async fn a_created_resource_is_handed_back_as_its_own_minted_arn() {
    // The rows differ in the resource created and, independently, in the region the
    // request is signed for: the ARN's region field comes from the credential scope.
    for region in ["us-west-2", "eu-west-1", "ap-south-1"] {
        for resource in minted_resources() {
            let case = format!("{} in {region}", resource.case);
            let emulator = emulator();
            let (status, created) =
                call(&emulator, region, resource.create_uri, &resource.request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to answer 200, got {status} {created:?}"
            );
            let arn = created
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let id = minted_id(&case, region, resource.resource_type, &arn);
            if let Some(submitted) = resource.submitted {
                assert_ne!(
                    id, submitted,
                    "case {case:?}: the id is the service's, not the value the request \
                     carried, got {arn:?}"
                );
            }

            // A second create of the same kind mints a second id, so one handle never
            // names two resources. The integration is one per regional endpoint, so
            // its second create is made in another region.
            let second_region = if resource.submitted.is_some() {
                region
            } else {
                "us-east-2"
            };
            let mut second_request = resource.request.clone();
            if let Some(Value::String(name)) = second_request.get_mut("Name") {
                *name = "projection-probe-two".to_string();
            }
            let (status, again) =
                call(&emulator, second_region, resource.create_uri, &second_request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the second create to answer 200, got {status} {again:?}"
            );
            let second_arn = again
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let second_id = minted_id(&case, second_region, resource.resource_type, &second_arn);
            assert_ne!(
                id, second_id,
                "case {case:?}: each create assigns its own id, got {second_arn:?} after {arn:?}"
            );

            // The handle resolves: the resource reads back under the ARN the create
            // answered, and reports that very ARN.
            let (read_uri, request, pointer) = if resource.submitted.is_some() {
                (
                    "/GetTelemetryPipeline",
                    json!({ "PipelineIdentifier": arn }),
                    "/Pipeline/Arn",
                )
            } else {
                ("/GetS3TableIntegration", json!({ "Arn": arn }), "/Arn")
            };
            let (status, read) = call(&emulator, region, read_uri, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read of {arn} to answer 200, got {status} {read:?}"
            );
            assert_eq!(
                read.pointer(pointer).and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?}: expected the read to report the handle {arn}, got {read:?}"
            );
        }
    }
}

// =============================================================================
// Rule 2 — one managed view per region
// =============================================================================

/// The managed view ARN the enrichment feature reports satisfies the member's own
/// modelled pattern, names one view per region, and is the view every later read of
/// that region reports.
#[tokio::test]
async fn the_enrichment_managed_view_is_one_minted_view_per_region() {
    // `AwsResourceExplorerManagedViewArn`'s own pattern, from
    // `smithy_query(query="shape", shape="GetTelemetryEnrichmentStatusOutput")`.
    let modelled = Regex::new(
        r"^arn:aws([a-z0-9\-]+)?:resource-explorer-2:([a-z0-9\-]+)?:([0-9]{12})?:managed-view/(.+)$",
    )
    .expect("the modelled pattern compiles");
    let emulator = emulator();
    let mut seen: Vec<String> = Vec::new();

    // The rows differ in the region the feature is started in: each region has its own
    // view, so no two of them can be the same value.
    for region in ["us-west-2", "eu-west-1", "ap-south-1"] {
        let (status, started) = call(&emulator, region, "/StartTelemetryEnrichment", &json!({})).await;
        assert_eq!(
            status, 202,
            "case {region:?}: expected the modelled 202, got {status} {started:?}"
        );
        let arn = started
            .get("AwsResourceExplorerManagedViewArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            modelled.is_match(&arn),
            "case {region:?}: expected a managed view ARN the modelled pattern admits, got {arn:?}"
        );
        assert!(
            !seen.contains(&arn),
            "case {region:?}: each region reads its own view, got the already-minted {arn:?}"
        );
        seen.push(arn.clone());

        // Stopping and restarting the feature reads the one view that region's first
        // start created: the view is a resource, not a per-call value.
        for uri in ["/StopTelemetryEnrichment", "/StartTelemetryEnrichment"] {
            let (status, answer) = call(&emulator, region, uri, &json!({})).await;
            assert_eq!(
                status, 202,
                "case {region:?} at {uri}: expected 202, got {status} {answer:?}"
            );
        }
        let (status, read) = call(
            &emulator,
            region,
            "/GetTelemetryEnrichmentStatus",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected the status read to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read.get("AwsResourceExplorerManagedViewArn")
                .and_then(Value::as_str),
            Some(arn.as_str()),
            "case {region:?}: expected the read to report the minted view {arn}, got {read:?}"
        );
    }
}

// =============================================================================
// Rule 3 — a read projects the status reason and the tag map
// =============================================================================

/// A pipeline read reports the reason for whatever status the pipeline is in, and a
/// tag map for a pipeline with no tags — an empty map is the tag set it has, which is
/// a different observation from a member the response never carried.
#[tokio::test]
async fn a_pipeline_read_reports_its_status_reason_and_its_tag_map() {
    // The rows differ in the tag set the create carried: none at all, one tag, and
    // several — which is what the "always a map" rule generalises over.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no tags at all", None),
        ("one tag", Some(json!({ "team": "observability" }))),
        (
            "several tags",
            Some(json!({ "team": "observability", "tier": "1", "env": "prod" })),
        ),
    ];

    for (index, (case, tags)) in rows.into_iter().enumerate() {
        let emulator = emulator();
        let name = format!("tag-projection-{index}");
        let mut request = json!({ "Name": name, "Configuration": { "Body": PIPELINE_BODY } });
        if let Some(tags) = &tags {
            request["Tags"] = tags.clone();
        }
        let (status, created) =
            call(&emulator, HOME_REGION, "/CreateTelemetryPipeline", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status} {created:?}"
        );

        let (status, read) = call(
            &emulator,
            HOME_REGION,
            "/GetTelemetryPipeline",
            &json!({ "PipelineIdentifier": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer 200, got {status} {read:?}"
        );
        assert_eq!(
            read.pointer("/Pipeline/Tags"),
            Some(&tags.unwrap_or_else(|| json!({}))),
            "case {case:?}: expected the pipeline's own tag set, got {read:?}"
        );
        let reason = read
            .pointer("/Pipeline/StatusReason/Description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let reported_status = read
            .pointer("/Pipeline/Status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !reason.is_empty(),
            "case {case:?}: the status {reported_status:?} is reported with the reason for \
             it, got {read:?}"
        );
        assert!(
            reason.to_lowercase().contains("pipeline"),
            "case {case:?}: the reason describes the resource it is about, got {reason:?}"
        );
    }
}

// =============================================================================
// Rule 4 — a member the request leaves to the service is recorded
// =============================================================================

/// One row of the source-type table: the rule body a create carries, and the
/// `TelemetrySourceTypes` every later read of that rule must report.
struct SourceTypes {
    case: &'static str,
    rule: Value,
    expected: Option<Value>,
}

/// `TelemetrySourceTypes` "must be correlated with the specific resource type", so a
/// create that leaves the member out has asked for the source type its resource type
/// correlates with, and that is what the record keeps.
///
/// The rows differ along what the rule generalises over: whether the request named its
/// own source types, which resource type it named (one the enum names a single source
/// type for, one it names several for, and one it names none for — the catalog is the
/// `ResourceType` / `TelemetrySourceType` enum pair, and the Route 53 Resolver row is
/// one the issue did not cite), which telemetry type it configures, and how many
/// source types the list carries.
fn source_type_rows() -> Vec<SourceTypes> {
    vec![
        SourceTypes {
            case: "a VPC log rule that names no source type",
            rule: json!({ "ResourceType": "AWS::EC2::VPC", "TelemetryType": "Logs" }),
            expected: Some(json!(["VPC_FLOW_LOGS"])),
        },
        SourceTypes {
            case: "a Route 53 Resolver log rule that names no source type",
            rule: json!({
                "ResourceType": "AWS::Route53Resolver::ResolverEndpoint",
                "TelemetryType": "Logs",
            }),
            expected: Some(json!(["ROUTE53_RESOLVER_QUERY_LOGS"])),
        },
        SourceTypes {
            case: "an EKS log rule, which five source types name, that names none",
            rule: json!({ "ResourceType": "AWS::EKS::Cluster", "TelemetryType": "Logs" }),
            expected: None,
        },
        SourceTypes {
            case: "a Lambda log rule, which no source type names",
            rule: json!({ "ResourceType": "AWS::Lambda::Function", "TelemetryType": "Logs" }),
            expected: None,
        },
        SourceTypes {
            case: "a VPC rule configuring metrics, which no log source type is",
            rule: json!({ "ResourceType": "AWS::EC2::VPC", "TelemetryType": "Metrics" }),
            expected: None,
        },
        SourceTypes {
            case: "a rule that names no resource type at all",
            rule: json!({ "TelemetryType": "Logs" }),
            expected: None,
        },
        SourceTypes {
            case: "a VPC log rule that names its one source type itself",
            rule: json!({
                "ResourceType": "AWS::EC2::VPC",
                "TelemetryType": "Logs",
                "TelemetrySourceTypes": ["VPC_FLOW_LOGS"],
            }),
            expected: Some(json!(["VPC_FLOW_LOGS"])),
        },
        SourceTypes {
            case: "an EKS log rule that names three of its source types, in its own order",
            rule: json!({
                "ResourceType": "AWS::EKS::Cluster",
                "TelemetryType": "Logs",
                "TelemetrySourceTypes": ["EKS_SCHEDULER_LOGS", "EKS_API_LOGS", "EKS_AUDIT_LOGS"],
            }),
            expected: Some(json!([
                "EKS_SCHEDULER_LOGS",
                "EKS_API_LOGS",
                "EKS_AUDIT_LOGS"
            ])),
        },
    ]
}

/// The telemetry source types a create settled on are what the rule read, the rule
/// listing and the rule an update replaced it with all report.
#[tokio::test]
async fn a_rule_reports_the_telemetry_source_types_its_create_settled_on() {
    for (index, row) in source_type_rows().into_iter().enumerate() {
        let emulator = emulator();
        let name = format!("source-types-{index}");
        let (status, created) = call(
            &emulator,
            HOME_REGION,
            "/CreateTelemetryRule",
            &json!({ "RuleName": name, "Rule": row.rule }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the create to answer 200, got {status} {created:?}",
            row.case
        );

        let (status, read) = call(
            &emulator,
            HOME_REGION,
            "/GetTelemetryRule",
            &json!({ "RuleIdentifier": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the read to answer 200, got {status} {read:?}",
            row.case
        );
        assert_eq!(
            read.pointer("/TelemetryRule/TelemetrySourceTypes"),
            row.expected.as_ref(),
            "case {:?}: expected the recorded source types, got {read:?}",
            row.case
        );

        let (status, listed) =
            call(&emulator, HOME_REGION, "/ListTelemetryRules", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the listing to answer 200, got {status} {listed:?}",
            row.case
        );
        let summary = listed
            .pointer("/TelemetryRuleSummaries/0")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            summary.get("TelemetrySourceTypes"),
            row.expected.as_ref(),
            "case {:?}: the summary reports the same source types the read does, got \
             {summary:?}",
            row.case
        );

        // An update records the rule it is given the same way, so the member the
        // update leaves out is settled again rather than dropped.
        let (status, updated) = call(
            &emulator,
            HOME_REGION,
            "/UpdateTelemetryRule",
            &json!({ "RuleIdentifier": name, "Rule": row.rule }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the update to answer 200, got {status} {updated:?}",
            row.case
        );
        let (_, reread) = call(
            &emulator,
            HOME_REGION,
            "/GetTelemetryRule",
            &json!({ "RuleIdentifier": name }),
        )
        .await;
        assert_eq!(
            reread.pointer("/TelemetryRule/TelemetrySourceTypes"),
            row.expected.as_ref(),
            "case {:?}: an update records the source types the way a create does, got \
             {reread:?}",
            row.case
        );
    }
}

// =============================================================================
// Rule 5 — one reported error per schema problem
// =============================================================================

/// One row of the schema-error table: the configuration document, and the
/// `(Reason, Message, FieldMap)` triples the answer must carry for it.
struct SchemaError {
    case: &'static str,
    body: &'static str,
    expected: Vec<(&'static str, String, Value)>,
}

/// A `FieldMap` as this table compares it: its JSON text, so one comparison covers the
/// keys AND the values it carries.
fn rendered(field_map: &Value) -> String {
    field_map.to_string()
}

/// The message a schema validator reports a required property that is not there with.
fn missing(property: &str) -> String {
    format!("required property '{property}' not found")
}

/// The message a schema validator reports a property it does not define with.
fn undefined(property: &str) -> String {
    format!(
        "property '{property}' is not defined in the schema and the schema does not allow \
         additional properties"
    )
}

/// Every reported error as a `(Reason, Message, FieldMap)` triple, sorted so a case
/// asserts the SET of problems rather than the order two independent checks ran in.
fn reported(case: &str, response: &Value) -> Vec<(String, String, String)> {
    let mut reported: Vec<(String, String, String)> = response
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|error| {
            let field_map = error.get("FieldMap").cloned().unwrap_or_else(|| {
                panic!("case {case:?}: every reported error carries a FieldMap, got {error:?}")
            });
            (
                error
                    .get("Reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                error
                    .get("Message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                rendered(&field_map),
            )
        })
        .collect();
    reported.sort();
    reported
}

/// A configuration document's problems are reported one error per problem, in the
/// vocabulary and the wording a schema validator reports them in: the property that is
/// required and not there, the property the schema does not define and the path it is
/// written at, and the property carrying more items than the schema allows.
///
/// The rows differ along what the rule generalises over: which problem the document
/// has, where in the document it is (the document's root, a named section, a position
/// of the processor sequence), how many problems one document has, and how the
/// document is spelled (YAML, and the JSON it is a superset of).
#[tokio::test]
async fn a_configuration_problem_is_reported_in_the_schemas_own_vocabulary() {
    let emulator = emulator();

    let rows: Vec<SchemaError> = vec![
        SchemaError {
            case: "a document that declares no pipeline at all",
            body: "{\"not-a-pipeline\": true}",
            expected: vec![
                (
                    "UNKNOWN_PROPERTY",
                    undefined("not-a-pipeline"),
                    json!({ "path": "root" }),
                ),
                ("MISSING_FIELD", missing("source"), json!({ "source": "root" })),
                ("MISSING_FIELD", missing("sink"), json!({ "sink": "root" })),
            ],
        },
        SchemaError {
            case: "a top-level pipeline with no sink",
            body: "source:\n  http: {}\n",
            expected: vec![("MISSING_FIELD", missing("sink"), json!({ "sink": "root" }))],
        },
        SchemaError {
            case: "a named section with a property the schema does not define",
            body: "log-pipeline:\n  source:\n    http: {}\n  sink:\n    - cloudwatch_logs: {}\n  \
                   retries: 3\n",
            expected: vec![(
                "UNKNOWN_PROPERTY",
                undefined("retries"),
                json!({ "path": "log-pipeline" }),
            )],
        },
        SchemaError {
            case: "a named section missing its source",
            body: "log-pipeline:\n  sink:\n    - cloudwatch_logs: {}\n",
            expected: vec![(
                "MISSING_FIELD",
                missing("source"),
                json!({ "source": "log-pipeline" }),
            )],
        },
        SchemaError {
            case: "a section whose source property names no plugin",
            body: "source:\nsink:\n  - cloudwatch_logs: {}\n",
            expected: vec![(
                "PIPELINE_CONFIGURATION_VALIDATION_ERROR",
                "must have at least 1 items but found 0".to_string(),
                json!({ "source": "root" }),
            )],
        },
        SchemaError {
            case: "a section naming two sinks where the schema allows one",
            body: "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n  - s3: {}\n",
            expected: vec![(
                "PIPELINE_CONFIGURATION_VALIDATION_ERROR",
                "must have at most 1 items but found 2".to_string(),
                json!({ "sink": "root" }),
            )],
        },
        SchemaError {
            case: "a section naming three sources where the schema allows one",
            body: "source:\n  http: {}\n  s3: {}\n  otel_logs: {}\nsink:\n  \
                   - cloudwatch_logs: {}\n",
            expected: vec![(
                "PIPELINE_CONFIGURATION_VALIDATION_ERROR",
                "must have at most 1 items but found 3".to_string(),
                json!({ "source": "root" }),
            )],
        },
        SchemaError {
            case: "a document with a problem at its root AND one in its section",
            body: "{\"owner\": \"team\", \"log-pipeline\": {\"source\": {\"http\": {}}}}",
            expected: vec![
                (
                    "UNKNOWN_PROPERTY",
                    undefined("owner"),
                    json!({ "path": "root" }),
                ),
                (
                    "MISSING_FIELD",
                    missing("sink"),
                    json!({ "sink": "log-pipeline" }),
                ),
            ],
        },
    ];

    for row in rows {
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            "/ValidateTelemetryPipelineConfiguration",
            &json!({ "Configuration": { "Body": row.body } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: a problem is reported, not refused; got {status} {response:?}",
            row.case
        );
        let mut expected: Vec<(String, String, String)> = row
            .expected
            .into_iter()
            .map(|(reason, message, field_map)| {
                (reason.to_string(), message, rendered(&field_map))
            })
            .collect();
        expected.sort();
        assert_eq!(
            reported(row.case, &response),
            expected,
            "case {:?}: expected those problems for {:?}, got {response:?}",
            row.case,
            row.body
        );
    }
}

/// One row of the processor-position table: the document, and the `(path, what the
/// message must name)` pair expected for each position at fault.
struct ProcessorPositions {
    case: &'static str,
    body: &'static str,
    expected: Vec<(&'static str, &'static str)>,
}

/// Every POSITION of a processor sequence this emulator cannot run is reported on its
/// own: its own entry, naming what is wrong and the path of that position, under this
/// emulator's own category — the real service's processor catalogue is not in the
/// model, so a name is never reported as one the schema does not define.
///
/// The rows differ along what the rule generalises over: how many positions are at
/// fault, where they are (one section, or one position in each of two chained
/// sections), and whether the fault is the processor or one of its options.
#[tokio::test]
async fn every_processor_position_this_emulator_cannot_run_is_reported_on_its_own() {
    let emulator = emulator();

    let rows: Vec<ProcessorPositions> = vec![
        ProcessorPositions {
            case: "one position of a top-level section",
            body: "source:\n  http: {}\nprocessor:\n  - grok: {}\nsink:\n  \
                   - cloudwatch_logs: {}\n",
            expected: vec![("processor[0]", "grok")],
        },
        ProcessorPositions {
            case: "two positions of one section",
            body: "source:\n  http: {}\nprocessor:\n  - grok: {}\n  - geoip: {}\nsink:\n  \
                   - cloudwatch_logs: {}\n",
            expected: vec![("processor[0]", "grok"), ("processor[1]", "geoip")],
        },
        ProcessorPositions {
            case: "an option the processor does not take",
            body: "source:\n  http: {}\nprocessor:\n  - csv:\n      delete_source: true\nsink:\n  \
                   - cloudwatch_logs: {}\n",
            expected: vec![("processor[0]", "delete_source")],
        },
        ProcessorPositions {
            case: "one position in each of two chained sections",
            body: "entry:\n  source:\n    http: {}\n  processor:\n    - grok: {}\n  sink:\n    \
                   - pipeline:\n        name: exit\nexit:\n  source:\n    pipeline:\n      \
                   name: entry\n  processor:\n    - csv:\n        delete_source: true\n  \
                   sink:\n    - cloudwatch_logs: {}\n",
            expected: vec![
                ("entry.processor[0]", "grok"),
                ("exit.processor[0]", "delete_source"),
            ],
        },
        ProcessorPositions {
            case: "a sequence this emulator runs, which is no defect at all",
            body: "source:\n  http: {}\nprocessor:\n  - parse_json: {}\n  - trim_string:\n      \
                   with_keys: [\"message\"]\nsink:\n  - cloudwatch_logs: {}\n",
            expected: vec![],
        },
    ];

    for row in rows {
        let (case, body) = (row.case, row.body);
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            "/ValidateTelemetryPipelineConfiguration",
            &json!({ "Configuration": { "Body": body } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a problem is reported, not refused; got {status} {response:?}"
        );
        let errors = response
            .get("Errors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut positions: Vec<(String, String)> = errors
            .iter()
            .filter(|error| {
                error.get("Reason").and_then(Value::as_str) == Some("UNSUPPORTED_PROCESSOR")
            })
            .map(|error| {
                (
                    error
                        .pointer("/FieldMap/path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    error
                        .get("Message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        positions.sort();
        let mut expected = row.expected;
        expected.sort();
        assert_eq!(
            positions.len(),
            expected.len(),
            "case {case:?}: expected one entry per position at fault, got {errors:?}"
        );
        for ((path, message), (expected_path, needle)) in positions.iter().zip(expected) {
            assert_eq!(
                path, expected_path,
                "case {case:?}: expected the entry to name the position {expected_path}, got \
                 {errors:?}"
            );
            assert!(
                message.contains(needle),
                "case {case:?}: expected the message at {path} to name {needle:?}, got \
                 {message:?}"
            );
        }
    }
}

/// The reported `FieldMap` is a map, whatever the problem: every entry a validate
/// answers carries one, and never an empty one — which is what lets a console point at
/// the property a problem is about.
#[tokio::test]
async fn every_reported_problem_carries_a_field_map() {
    let emulator = emulator();
    let (status, response) = call(
        &emulator,
        HOME_REGION,
        "/ValidateTelemetryPipelineConfiguration",
        &json!({
            "Configuration": {
                "Body": "owner: team\nlog-pipeline:\n  processor:\n    - grok: {}\n",
            }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");
    let errors = response
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        errors.len() >= 3,
        "expected one error per problem of the document, got {errors:?}"
    );
    for error in &errors {
        let field_map = error
            .get("FieldMap")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(Map::new);
        assert!(
            !field_map.is_empty()
                && field_map
                    .values()
                    .all(|value| value.as_str().is_some_and(|value| !value.is_empty())),
            "expected a non-empty FieldMap of strings, got {error:?}"
        );
    }
}
