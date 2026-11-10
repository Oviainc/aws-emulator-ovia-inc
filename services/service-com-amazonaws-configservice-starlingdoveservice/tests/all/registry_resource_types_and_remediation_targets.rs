// SPDX-License-Identifier: CC0-1.0

//! Two rules about values Config resolves against something OUTSIDE its own store, end to
//! end over the wire, plus the wording of the refusal a rule that holds a remediation
//! configuration earns.
//!
//! 1. A discovered-resource LISTING is asked about a resource type NAME: a type this
//!    service's model enumerates, or a CloudFormation registry type name (the name a
//!    custom resource type is registered, and its configuration state recorded, under).
//!    Either spelling is answered from the inventory — the resources held under that exact
//!    name, an empty list when it holds none — and only a value that names no resource
//!    type in either sense is refused, with Config's own unsupported-resource-type
//!    sentence.
//! 2. A remediation configuration's TARGET must exist: a `SSM_DOCUMENT` `TargetId` is
//!    resolved against Systems Manager's own document registry, and a configuration naming
//!    a document that registry does not hold is refused `InvalidParameterValueException`
//!    with AWS's "remediation target … could not be found" sentence, before anything is
//!    persisted.
//! 3. `DeleteConfigRule` refuses a rule that still holds a remediation configuration with
//!    AWS's own `ResourceInUseException` sentence, and `DeleteRemediationConfiguration`
//!    refuses a rule that holds none with its own: each carries the names the request sent
//!    interpolated into fixed text, unquoted.
//!
//! Rule 2 is driven against Systems Manager ITSELF: Config and SSM are mounted on one
//! engine, and the documents the rows name are the ones SSM publishes plus ones created
//! and deleted through SSM's own API — so what makes a target resolvable is the other
//! service's state and not a list inside this crate.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry,
};
use service_com_amazonaws_configservice_starlingdoveservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for, which is the Region Config asks Systems Manager
/// about a document in.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of this service (its `OPERATIONS.md` wire-protocol section).
const CONFIG_TARGET_PREFIX: &str = "StarlingDoveService";

/// The `X-Amz-Target` prefix of Systems Manager (that crate's `OPERATIONS.md`).
const SSM_TARGET_PREFIX: &str = "AmazonSSM";

/// The sentence Config answers a value that names no resource type with, spelled out here
/// rather than read from the service's own constant so this suite PINS the text.
const UNSUPPORTED_RESOURCE_TYPE: &str =
    "The resource type you specified is not supported by AWS Config. Verify the resource \
     type, and try again.";

/// A document Systems Manager PUBLISHES, so a target naming it resolves without anybody
/// creating it (the Systems Manager Automation runbook reference).
const PUBLISHED_RUNBOOK: &str = "AWS-PublishSNSNotification";

/// Another published runbook, so the positive control is not read off one name.
const OTHER_PUBLISHED_RUNBOOK: &str = "AWS-ConfigureS3BucketVersioning";

/// The only published `RemediationTargetType` value, from `smithy_query(query="shape",
/// shape="RemediationTargetType")`.
const SSM_DOCUMENT: &str = "SSM_DOCUMENT";

/// Config and Systems Manager over ONE engine, reached the way a client reaches them.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// One RPC call: `POST /` carrying `X-Amz-Target`, signed for `signing_name` in
    /// `region` — the Region the credential scope names, which is the one a handler reads
    /// a regional resource under.
    async fn call_in_region(
        &self,
        region: &str,
        signing_name: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{region}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// [`Self::call_in_region`] in the Region every row signs for.
    async fn call(
        &self,
        signing_name: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        self.call_in_region(REGION, signing_name, target_prefix, operation, body)
            .await
    }

    /// A Config call.
    async fn config(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call("config", CONFIG_TARGET_PREFIX, operation, body)
            .await
    }

    /// A Config call required to answer 200.
    async fn config_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.config(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
        );
        parsed
    }

    /// A Systems Manager call required to answer 200.
    async fn ssm_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.ssm_ok_in_region(case, REGION, operation, body).await
    }

    /// A Systems Manager call in `region`, required to answer 200.
    async fn ssm_ok_in_region(
        &self,
        case: &str,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> Value {
        let (status, parsed) = self
            .call_in_region(region, "ssm", SSM_TARGET_PREFIX, operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation} in {region}, got {status} with \
             {parsed}"
        );
        parsed
    }
}

/// The wire error code of a refusal body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered.rsplit('#').next().unwrap_or(rendered).to_string()
}

/// The message of a refusal body.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// Put the customer managed configuration recorder a discovered-resource read needs.
async fn with_recorder(emulator: &Emulator) {
    emulator
        .config_ok(
            "setup",
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "roleARN": "arn:aws:iam::000000000000:role/config-role",
                },
            }),
        )
        .await;
}

/// Seed the discovered-resource inventory with `(resource type, resource id)` pairs, the
/// way the other suites of this crate seed state: through the store a handler reaches.
///
/// Seeding is what lets a listing of a CloudFormation registry type name be checked
/// against something other than emptiness: `PutResourceConfig` records only a type the
/// registry holds a registration for, so the inventory of a custom type is written here.
fn seed_inventory(emulator: &Emulator, resources: &[(&str, &str)]) {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    for (resource_type, resource_id) in resources {
        store.discovered_resources.insert(
            ((*resource_type).to_string(), (*resource_id).to_string()),
            DiscoveredResourceEntry {
                resource_type: (*resource_type).to_string(),
                resource_id: (*resource_id).to_string(),
                ..DiscoveredResourceEntry::default()
            },
        );
    }
}

/// The `resourceId` of every identifier `ListDiscoveredResources` reported.
fn listed_ids(case: &str, body: &Value) -> Vec<String> {
    body["resourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected resourceIdentifiers, got {body}"))
        .iter()
        .map(|identifier| {
            identifier["resourceId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an identifier has no id, got {body}"))
                .to_string()
        })
        .collect()
}

/// The `ResourceId` of every identifier `ListAggregateDiscoveredResources` reported.
fn aggregated_ids(case: &str, body: &Value) -> Vec<String> {
    body["ResourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ResourceIdentifiers, got {body}"))
        .iter()
        .map(|identifier| {
            identifier["ResourceId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an identifier has no id, got {body}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_discovered_resource_listing_is_asked_about_a_resource_type_name() {
    let emulator = Emulator::new();
    with_recorder(&emulator).await;
    emulator
        .config_ok(
            "setup",
            "PutConfigurationAggregator",
            &json!({
                "ConfigurationAggregatorName": "every-source",
                "AccountAggregationSources": [{
                    "AccountIds": [emulator_server::emulator_account_id()],
                    "AllAwsRegions": true,
                }],
            }),
        )
        .await;

    // The inventory: one resource under a type the model enumerates, and one under a
    // CloudFormation registry type name, which is what a custom resource type is
    // recorded under.
    let enumerated = "AWS::S3::Bucket";
    let registry_type = "Sim2Real::FidelityProbe::Widget";
    seed_inventory(
        &emulator,
        &[(enumerated, "bucket-1"), (registry_type, "widget-1")],
    );

    // Rows differ along the SPELLING of the type name asked about and along what the
    // inventory holds under it: types the model enumerates (one with a resource, one
    // without), registry type names (one with a resource, several without — a
    // third-party organization, one at the published minimum length of a registry type
    // name, a module type, and a modelled type mis-cased, which is a type name Config
    // records nothing under). Sources: the value set of
    // `com.amazonaws.configservice#ResourceType` for the enumerated rows, and the
    // CloudFormation `TypeName` pattern (`smithy_query(query="shape", shape="TypeName")`
    // on that service's model) for the registry rows.
    let rows: [(&str, &str, &[&str]); 8] = [
        ("an enumerated type with a resource", enumerated, &["bucket-1"]),
        ("an enumerated type with none", "AWS::EC2::Volume", &[]),
        ("another enumerated type with none", "AWS::IAM::Role", &[]),
        ("a registry type with a resource", registry_type, &["widget-1"]),
        ("a registry type with none", "Acme::Network::Switch", &[]),
        ("a registry type at the minimum length", "Ab::Cd::Ef", &[]),
        ("a registry module type", "My::Name::Space::MODULE", &[]),
        ("an enumerated type mis-cased", "aws::s3::bucket", &[]),
    ];

    for (case, resource_type, expected) in rows {
        let body = emulator
            .config_ok(
                case,
                "ListDiscoveredResources",
                &json!({ "resourceType": resource_type }),
            )
            .await;
        let listed = listed_ids(case, &body);
        assert_eq!(
            listed, expected,
            "case {case:?} ({resource_type}): expected the ids {expected:?}, got {listed:?}"
        );

        // The aggregate listing is asked the same question and answers from the same
        // inventory, so the rule holds on both discovered-resource listings.
        let body = emulator
            .config_ok(
                case,
                "ListAggregateDiscoveredResources",
                &json!({
                    "ConfigurationAggregatorName": "every-source",
                    "ResourceType": resource_type,
                }),
            )
            .await;
        let aggregated = aggregated_ids(case, &body);
        assert_eq!(
            aggregated, expected,
            "case {case:?} ({resource_type}): expected the aggregated ids {expected:?}, got \
             {aggregated:?}"
        );
    }

    // A value that NAMES no resource type — in the modelled set or in the registry's
    // form — is still refused, and with Config's own sentence. Rows differ along how the
    // value misses the form: characters no segment carries, too few segments, an empty
    // segment, segments shorter than the form admits, and the empty value.
    for value in [
        "not-a-resource-type",
        "AWS::S3",
        "AWS::::Account",
        "A::B::C",
        "",
    ] {
        for (operation, body) in [
            (
                "ListDiscoveredResources",
                json!({ "resourceType": value }),
            ),
            (
                "ListAggregateDiscoveredResources",
                json!({
                    "ConfigurationAggregatorName": "every-source",
                    "ResourceType": value,
                }),
            ),
        ] {
            let case = format!("{operation} carrying {value:?}");
            let (status, parsed) = emulator.config(operation, &body).await;
            assert_eq!(
                (status, error_code(&parsed), error_message(&parsed)),
                (
                    400,
                    "ValidationException".to_string(),
                    UNSUPPORTED_RESOURCE_TYPE.to_string()
                ),
                "case {case:?}: expected 400 ValidationException with Config's \
                 unsupported-resource-type sentence, got {status} with {parsed}"
            );
        }
    }
}

/// The `PutRemediationConfigurations` body for one configuration of `rule` targeting the
/// document `target_id`.
fn remediation(rule: &str, target_id: &str) -> Value {
    json!({
        "RemediationConfigurations": [{
            "ConfigRuleName": rule,
            "TargetType": SSM_DOCUMENT,
            "TargetId": target_id,
        }],
    })
}

/// The `TargetId` of every remediation configuration the account holds for `rule`.
async fn described_targets(emulator: &Emulator, case: &str, rule: &str) -> Vec<String> {
    let body = emulator
        .config_ok(
            case,
            "DescribeRemediationConfigurations",
            &json!({ "ConfigRuleNames": [rule] }),
        )
        .await;
    body["RemediationConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected RemediationConfigurations, got {body}"))
        .iter()
        .map(|configuration| {
            configuration["TargetId"]
                .as_str()
                .unwrap_or("<no TargetId>")
                .to_string()
        })
        .collect()
}

/// An Automation runbook body, the shape a Systems Manager Automation document declares.
fn automation_content(description: &str) -> String {
    json!({
        "schemaVersion": "0.3",
        "description": description,
        "assumeRole": "{{ AutomationAssumeRole }}",
        "parameters": {
            "AutomationAssumeRole": { "type": "String" },
            "BucketName": { "type": "String" },
        },
        "mainSteps": [{
            "name": "putBucketVersioning",
            "action": "aws:executeAwsApi",
            "inputs": { "Service": "s3", "Api": "PutBucketVersioning" },
        }],
    })
    .to_string()
}

#[tokio::test]
async fn a_remediation_target_is_resolved_against_systems_managers_document_registry() {
    let emulator = Emulator::new();
    with_recorder(&emulator).await;

    // A document created through Systems Manager's own API, so a resolvable target is not
    // only one SSM publishes — and the same document created in ANOTHER Region, which this
    // endpoint's registry does not hold, so what makes a target resolvable is the registry
    // Config asks rather than the name alone.
    let created = "Sim2Real-OwnRunbook";
    let elsewhere = "Sim2Real-ElsewhereRunbook";
    emulator
        .ssm_ok(
            "setup",
            "CreateDocument",
            &json!({
                "Name": created,
                "DocumentType": "Automation",
                "Content": automation_content("a remediation this account owns"),
            }),
        )
        .await;
    emulator
        .ssm_ok_in_region(
            "setup",
            "us-west-2",
            "CreateDocument",
            &json!({
                "Name": elsewhere,
                "DocumentType": "Automation",
                "Content": automation_content("a remediation in another Region"),
            }),
        )
        .await;

    // Rows differ along WHETHER Systems Manager holds the document the target names at the
    // endpoint Config asks, and along how it came to hold it or not: a published runbook, a
    // second published runbook, a document this account created here, the same kind of
    // document created in another Region, a published name misspelled, a published name
    // mis-cased (a document name is case-sensitive), a name nothing ever created, and the
    // ARN form of a document nothing created ("The name can be an Amazon Resource Name
    // (ARN)").
    let rows: [(&str, &str, bool); 8] = [
        ("a published runbook", PUBLISHED_RUNBOOK, true),
        ("another published runbook", OTHER_PUBLISHED_RUNBOOK, true),
        ("a runbook this account created", created, true),
        ("a runbook created in another Region", elsewhere, false),
        ("a published name misspelled", "AWS-PublishSNSNotifications", false),
        ("a published name mis-cased", "aws-publishsnsnotification", false),
        ("a name nothing created", "Sim2Real-NoSuchAutomationDocument", false),
        (
            "the ARN of a document nothing created",
            "arn:aws:ssm:us-east-1:000000000000:document/Sim2Real-NoSuchArnDocument",
            false,
        ),
    ];

    for (index, (case, target_id, resolvable)) in rows.into_iter().enumerate() {
        // One rule per row, so a row's acceptance is read back against its own rule.
        let rule = format!("remediation-target-rule-{index}");
        emulator
            .config_ok(
                case,
                "PutConfigRule",
                &json!({
                    "ConfigRule": {
                        "ConfigRuleName": &rule,
                        "Source": {
                            "Owner": "AWS",
                            "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED",
                        },
                    },
                }),
            )
            .await;

        let (status, parsed) = emulator
            .config(
                "PutRemediationConfigurations",
                &remediation(&rule, target_id),
            )
            .await;

        if resolvable {
            assert_eq!(
                status, 200,
                "case {case:?} ({target_id}): a target Systems Manager holds is accepted, got \
                 {status} with {parsed}"
            );
            assert_eq!(
                parsed["FailedBatches"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: an accepted configuration fails no batch, got {parsed}"
            );
            // Read back through the operation that reports it: the configuration is
            // persisted with the target it was sent with.
            let targets = described_targets(&emulator, case, &rule).await;
            assert_eq!(
                targets,
                vec![target_id.to_string()],
                "case {case:?}: expected the stored target {target_id:?}, got {targets:?}"
            );
        } else {
            let expected = format!(
                "The remediation target for {rule} could not be found. Specify the \
                 appropriate remediation target and try again."
            );
            assert_eq!(
                (status, error_code(&parsed), error_message(&parsed)),
                (400, "InvalidParameterValueException".to_string(), expected),
                "case {case:?} ({target_id}): expected 400 InvalidParameterValueException \
                 naming the rule, got {status} with {parsed}"
            );
            // Nothing is persisted: the refused configuration is reported by no read.
            let targets = described_targets(&emulator, case, &rule).await;
            assert!(
                targets.is_empty(),
                "case {case:?}: the refused configuration must not be stored, got {targets:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_rule_holding_a_remediation_configuration_is_refused_in_awss_wording() {
    let emulator = Emulator::new();
    with_recorder(&emulator).await;

    // Rows differ along the rule NAME, which is the one variable part of the sentence:
    // names differing in length, in case and in the characters `ConfigRuleName` admits.
    for rule in [
        "a-rule",
        "RULE.With.Dots",
        "rule_7",
        &"long-rule-name".repeat(4),
    ] {
        emulator
            .config_ok(
                rule,
                "PutConfigRule",
                &json!({
                    "ConfigRule": {
                        "ConfigRuleName": rule,
                        "Source": {
                            "Owner": "AWS",
                            "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED",
                        },
                    },
                }),
            )
            .await;
        emulator
            .config_ok(
                rule,
                "PutRemediationConfigurations",
                &remediation(rule, PUBLISHED_RUNBOOK),
            )
            .await;

        let (status, parsed) = emulator
            .config("DeleteConfigRule", &json!({ "ConfigRuleName": rule }))
            .await;
        let expected = format!(
            "The rule {rule} currently has a RemediationConfiguration. Please delete the \
             RemediationConfiguration and try again."
        );
        assert_eq!(
            (status, error_code(&parsed), error_message(&parsed)),
            (400, "ResourceInUseException".to_string(), expected),
            "case {rule:?}: expected 400 ResourceInUseException in AWS's wording, got {status} \
             with {parsed}"
        );

        // Once the remediation configuration is gone the same rule deletes, so the
        // refusal is about what the rule HOLDS and not about the rule.
        emulator
            .config_ok(
                rule,
                "DeleteRemediationConfiguration",
                &json!({ "ConfigRuleName": rule }),
            )
            .await;

        // And the refusal for a rule that holds none carries the rule's name — and the
        // resource type where the request names one — the same way: interpolated into
        // Config's fixed text, unquoted.
        for (named_type, expected) in [
            (
                None,
                format!("No RemediationConfiguration for rule {rule} exists"),
            ),
            (
                Some("AWS::S3::Bucket"),
                format!(
                    "No RemediationConfiguration for rule {rule} and resource type \
                     AWS::S3::Bucket exists"
                ),
            ),
        ] {
            let mut body = json!({ "ConfigRuleName": rule });
            if let Some(named_type) = named_type {
                body["ResourceType"] = json!(named_type);
            }
            let (status, parsed) = emulator.config("DeleteRemediationConfiguration", &body).await;
            assert_eq!(
                (status, error_code(&parsed), error_message(&parsed)),
                (
                    400,
                    "NoSuchRemediationConfigurationException".to_string(),
                    expected
                ),
                "case {rule:?} ({named_type:?}): expected 400 \
                 NoSuchRemediationConfigurationException in AWS's wording, got {status} with \
                 {parsed}"
            );
        }

        emulator
            .config_ok(rule, "DeleteConfigRule", &json!({ "ConfigRuleName": rule }))
            .await;
    }
}
