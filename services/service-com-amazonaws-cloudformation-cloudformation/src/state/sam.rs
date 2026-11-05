// SPDX-License-Identifier: CC0-1.0

//! The `AWS::Serverless-2016-10-31` transform: expanding a SAM template into the
//! CloudFormation resources it stands for.
//!
//! "The optional Transform section … specifies one or more macros that CloudFormation uses
//! to process your template", and CloudFormation processes those macros BEFORE it does
//! anything with the template's `Resources`: a change set plans, and a stack deploys, the
//! PROCESSED template. `AWS::Serverless-2016-10-31` is the macro the AWS Serverless
//! Application Model is published as, and what it processes is every `AWS::Serverless::*`
//! resource — a type that exists only as that macro's INPUT, never as a resource
//! CloudFormation creates. So a template declaring the transform is expanded here, by
//! [`expand`], before [`super::template::parse`] reads a single resource out of it; every
//! reader of a template body (a change set's plan, a deployment's resources, a stack's
//! outputs) therefore sees the concrete resources and never the macro's input types.
//!
//! What the expansion is, per the SAM resource reference ("AWS SAM resource and property
//! reference" and "Generated AWS CloudFormation resources" in the AWS SAM Developer
//! Guide):
//!
//! * `AWS::Serverless::Function` becomes an `AWS::Lambda::Function`, plus the
//!   `AWS::IAM::Role` it executes as when the declaration names no `Role` of its own, plus
//!   one resource per event the function is triggered by — an `AWS::Lambda::Permission` for
//!   each API event, an `AWS::Lambda::EventSourceMapping` for each stream or queue event,
//!   an `AWS::Events::Rule` for each schedule or event-pattern event, an
//!   `AWS::SNS::Subscription` for each topic event.
//! * An `Api` event that names no `RestApiId` is an IMPLICIT API: the transform synthesizes
//!   ONE `AWS::ApiGateway::RestApi` (`ServerlessRestApi`) whose OpenAPI body carries the
//!   path and method of every such event in the template, and deploys it — an
//!   `AWS::ApiGateway::Deployment` and an `AWS::ApiGateway::Stage` for the implicit `Prod`
//!   stage. A `HttpApi` event does the same through `AWS::ApiGatewayV2::Api` and its
//!   `$default` stage.
//! * `AWS::Serverless::Api` and `AWS::Serverless::HttpApi` become that same API trio from
//!   the definition the declaration states.
//! * A declaration's `Policies` is what the role generated for it grants: every entry that
//!   is not an AWS managed policy becomes one INLINE policy on that role, named for the
//!   declaration and the entry's position ([`role_policies`]), and the role carries the
//!   managed policy each of the function's event sources needs
//!   ([`EVENT_SOURCE_POLICIES`]).
//! * `AWS::Serverless::SimpleTable` becomes an `AWS::DynamoDB::Table`,
//!   `AWS::Serverless::LayerVersion` an `AWS::Lambda::LayerVersion` and
//!   `AWS::Serverless::StateMachine` an `AWS::StepFunctions::StateMachine` (plus its
//!   execution role).
//!
//! ## A deployment's logical id carries a content hash
//!
//! An `AWS::ApiGateway::Deployment` is immutable once created, so "AWS SAM … creates a new
//! deployment whenever the API's definition changes" by giving each deployment a logical id
//! of its own: the deployment's logical id is `<api>Deployment` followed by a hash of the
//! API's definition ([`DIGEST_CHARS`] hex characters), and the stage points at that id. The
//! same definition therefore plans the same single deployment, and an edited definition
//! plans a different one — which is the property [`digest`] gives it. The DIGEST FUNCTION
//! is not observable (nothing reads the hash back), so this module uses the crate's
//! existing SHA-256; what is observable is the shape of the id and that it is a function of
//! the definition's CONTENT, both of which hold here. An
//! `AWS::Serverless::LayerVersion` carries the same content hash on its own logical id, for
//! the same reason: a layer version cannot be updated in place. Both are derived through the
//! crate's one digest derivation ([`super::template::content_digest`]).
//!
//! An expansion that RENAMES a declaration re-points every reference to it
//! ([`rename_references`]): CloudFormation plans and deploys the macro's OUTPUT, so a `Ref`,
//! an `Fn::GetAtt`, an `Fn::Sub` variable or a `DependsOn` naming the declaration's own
//! logical id has to name the expanded one, in every section of the processed template.
//! Nothing else is renamed, so every other reference reads exactly as the template wrote it.
//!
//! ## What is left as declared
//!
//! An `AWS::Serverless::Application` resolves its template from the AWS Serverless
//! Application Repository, which needs an outbound connection this emulator does not make,
//! and an `AWS::Serverless::GraphQLApi` or `AWS::Serverless::Connector` expansion is not
//! modelled here: those declarations are left exactly as the template spells them rather
//! than expanded into resources nothing computed. The same holds for an event type this
//! module does not carry — it contributes no resource, instead of a fabricated one. Every
//! OTHER macro (`AWS::Include`, a private one) is still unexpanded: this module answers for
//! the SAM transform alone, which is why [`super::resource_types`] keeps admitting the
//! types a template declaring any transform spells.
//!
//! ## Bounds
//!
//! The expansion is driven by a template body that is already bounded
//! ([`super::template::parse`] refuses an over-long body, too many nodes and too much
//! nesting before this module sees it), and it adds a bounded number of resources per
//! declaration, so no caller-controlled input can make it loop or recurse unboundedly: the
//! one recursive walk here ([`canonical`], over a value being hashed) carries its own depth
//! bound. The documented per-template resource quota is applied to the EXPANDED template,
//! which is the one CloudFormation validates.

use std::collections::BTreeMap;

use ::serde_json::{json, Map, Value};

use super::template::{declared_transforms, MAX_TEMPLATE_DEPTH, PARTITION};

/// The policy templates a `Policies` entry's shorthand can name, and the policy document
/// each one stands for.
mod policy_templates;

/// The macro name the AWS Serverless Application Model is published under, as a template's
/// `Transform` section names it.
pub const TRANSFORM: &str = "AWS::Serverless-2016-10-31";

/// The resource types the transform expands, spelled as the SAM resource reference spells
/// them.
const FUNCTION: &str = "AWS::Serverless::Function";
const API: &str = "AWS::Serverless::Api";
const HTTP_API: &str = "AWS::Serverless::HttpApi";
const SIMPLE_TABLE: &str = "AWS::Serverless::SimpleTable";
const LAYER_VERSION: &str = "AWS::Serverless::LayerVersion";
const STATE_MACHINE: &str = "AWS::Serverless::StateMachine";

/// The logical id of the REST API the transform synthesizes for the `Api` events that name
/// no `RestApiId` ("AWS SAM creates … an implicit API with the logical ID
/// `ServerlessRestApi`").
const IMPLICIT_REST_API: &str = "ServerlessRestApi";

/// The logical id of the HTTP API synthesized for `HttpApi` events that name no `ApiId`.
const IMPLICIT_HTTP_API: &str = "ServerlessHttpApi";

/// The stage the implicit REST API is deployed to ("the implicit API … is deployed to a
/// stage called `Prod`").
const IMPLICIT_STAGE: &str = "Prod";

/// The stage an HTTP API is deployed to by default, and the suffix its logical id carries:
/// `AWS::ApiGatewayV2` deploys to the `$default` stage, whose name is not a logical-id
/// fragment.
const HTTP_DEFAULT_STAGE: &str = "$default";
const HTTP_DEFAULT_STAGE_SUFFIX: &str = "ApiGatewayDefaultStage";

/// How many hex characters of the definition's digest a deployment's (or a layer version's)
/// logical id carries.
const DIGEST_CHARS: usize = 10;

/// `AWS::Serverless::LayerVersion$RetentionPolicy`: "specifies whether old versions of your
/// layer version are retained or deleted after an update", whose values are the
/// `DeletionPolicy` attribute's own `Retain` and `Delete` and whose default is to RETAIN —
/// which is what the expansion states on the generated layer version, since the hashed
/// logical id above makes every content a resource of its own and so makes every content
/// change a REMOVAL of the resource the stack had.
const RETENTION_POLICY_PROPERTY: &str = "RetentionPolicy";

/// The resource attributes a reference walk reads: the ways a template names another
/// resource ([`rename_references`]).
const REF: &str = "Ref";
const FN_SUB: &str = "Fn::Sub";
const DEPENDS_ON: &str = "DependsOn";
const DELETION_POLICY: &str = "DeletionPolicy";

/// What the content hash below is derived under, so it cannot be the same digits as another
/// identifier this crate derives from the same text (see
/// [`super::template::content_digest`]).
const DIGEST_SALT: &str = "sam/definition";

/// The tag the transform puts on the Lambda functions and roles it creates
/// (`lambda:createdBy: SAM`, as a synthesized function's tags carry).
const CREATED_BY_TAG: (&str, &str) = ("lambda:createdBy", "SAM");

/// The managed policy the execution role the transform creates carries: "AWS SAM creates
/// … an IAM role with the `AWSLambdaBasicExecutionRole` managed policy".
///
/// Spelled with the partition rather than `${AWS::Partition}`, which is how the recorded
/// expansion spells every ARN it writes: the transform runs in one partition and names it.
const BASIC_EXECUTION_POLICY: &str = "iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// The managed policy the role a function executes as needs for one EVENT SOURCE TYPE:
/// an event source the Lambda service READS FROM on the function's behalf is polled with
/// the function's own permissions, so the transform attaches the execution-role policy
/// published for that source ("AWS SAM … adds the permissions the event source needs to the
/// function's execution role").
///
/// Keyed by the event type an `Events` entry declares, spelled as
/// [`EVENT_SOURCE_MAPPINGS`] spells it. A pull source whose permissions the published
/// expansion grants as policy STATEMENTS rather than as one managed policy (an Amazon MQ
/// broker, a self-managed cluster, a DocumentDB change stream — each of which needs the
/// secret and the network of the resource it reads) has no entry here: nothing is attached
/// for it, rather than a policy nothing published.
const EVENT_SOURCE_POLICIES: &[(&str, &str)] = &[
    (
        "DynamoDB",
        "iam::aws:policy/service-role/AWSLambdaDynamoDBExecutionRole",
    ),
    (
        "Kinesis",
        "iam::aws:policy/service-role/AWSLambdaKinesisExecutionRole",
    ),
    (
        "SQS",
        "iam::aws:policy/service-role/AWSLambdaSQSQueueExecutionRole",
    ),
    (
        "MSK",
        "iam::aws:policy/service-role/AWSLambdaMSKExecutionRole",
    ),
];

/// The function timeout the transform states when the declaration states none (SAM's
/// documented default for `AWS::Serverless::Function$Timeout`, which is Lambda's own
/// default of 3 seconds).
const DEFAULT_TIMEOUT: u64 = 3;

/// The members an `AWS::Serverless::Function` and an `AWS::Lambda::Function` spell the same
/// way, which the expansion carries straight through.
const FUNCTION_PASSTHROUGH: &[&str] = &[
    "Architectures",
    "CodeSigningConfigArn",
    "Description",
    "Environment",
    "EphemeralStorage",
    "FileSystemConfigs",
    "FunctionName",
    "Handler",
    "ImageConfig",
    "KmsKeyArn",
    "Layers",
    "LoggingConfig",
    "MemorySize",
    "PackageType",
    "RecursiveLoop",
    "ReservedConcurrentExecutions",
    "Runtime",
    "RuntimeManagementConfig",
    "SnapStart",
    "Timeout",
    "VpcConfig",
];

/// The members an `AWS::Serverless::Api` and an `AWS::ApiGateway::RestApi` spell the same
/// way.
const REST_API_PASSTHROUGH: &[&str] = &[
    "BinaryMediaTypes",
    "Description",
    "DisableExecuteApiEndpoint",
    "EndpointConfiguration",
    "MinimumCompressionSize",
    "Mode",
    "Name",
    "Policy",
];

/// The members an `AWS::Serverless::Api`'s stage takes from the declaration, spelled the
/// same way on `AWS::ApiGateway::Stage`.
const REST_STAGE_PASSTHROUGH: &[&str] = &[
    "AccessLogSetting",
    "CacheClusterEnabled",
    "CacheClusterSize",
    "CanarySetting",
    "MethodSettings",
    "TracingEnabled",
    "Variables",
];

/// The members an `AWS::Serverless::SimpleTable` and an `AWS::DynamoDB::Table` spell the
/// same way.
const TABLE_PASSTHROUGH: &[&str] = &[
    "ProvisionedThroughput",
    "SSESpecification",
    "TableName",
    "PointInTimeRecoverySpecification",
];

/// The event types that become an `AWS::Lambda::EventSourceMapping`, each with the member
/// the event's source ARN is spelled under on that resource: a stream source is an
/// `EventSourceArn`, a self-managed broker a `SelfManagedEventSource`.
const EVENT_SOURCE_MAPPINGS: &[(&str, &str)] = &[
    ("SQS", "Queue"),
    ("DynamoDB", "Stream"),
    ("Kinesis", "Stream"),
    ("MSK", "Stream"),
    ("MQ", "Broker"),
    ("DocumentDB", "Cluster"),
];

/// The members an event-source-mapping event and `AWS::Lambda::EventSourceMapping` spell
/// the same way.
const EVENT_SOURCE_PASSTHROUGH: &[&str] = &[
    "BatchSize",
    "BisectBatchOnFunctionError",
    "DestinationConfig",
    "Enabled",
    "FilterCriteria",
    "FunctionResponseTypes",
    "MaximumBatchingWindowInSeconds",
    "MaximumRecordAgeInSeconds",
    "MaximumRetryAttempts",
    "ParallelizationFactor",
    "ScalingConfig",
    "StartingPosition",
    "StartingPositionTimestamp",
    "TumblingWindowInSeconds",
];

/// Expand the template document in place, where it declares [`TRANSFORM`].
///
/// Answers whether the expansion CHANGED anything — a document declaring any other
/// transform, or none, is left exactly as it was read, because this is the one macro this
/// emulator processes, and so is one whose declarations the macro has nothing to expand.
/// Its caller needs to know, because a processed template that differs from the submitted
/// one is the only reason to report anything but the submitted body itself.
pub fn expand(document: &mut Value) -> bool {
    if !declares_the_transform(document) {
        return false;
    }
    let Some(resources) = document.get("Resources").and_then(Value::as_object).cloned() else {
        return false;
    };
    let globals = document.get("Globals").cloned().unwrap_or(Value::Null);
    let expansion = expanded_resources(&resources, &globals);
    if expansion.resources == resources && expansion.renamed.is_empty() {
        return false;
    }
    match document.as_object_mut() {
        Some(sections) => {
            sections.insert("Resources".to_string(), Value::Object(expansion.resources));
            // `Globals` is the macro's own section, and the expansion resolved it into
            // every resource it applies to: the processed template no longer carries it.
            sections.remove("Globals");
            // A declaration the expansion RENAMED is named by its own logical id wherever
            // the template refers to it, and the template CloudFormation goes on to plan
            // and deploy is this one: every such reference is re-pointed at the id the
            // expansion gave the resource, in every section that carries one.
            for section in sections.values_mut() {
                rename_references(section, &expansion.renamed, 0);
            }
            true
        }
        None => false,
    }
}

/// What the transform made of a template's `Resources` section.
#[derive(Debug, Default)]
struct Expansion {
    /// The `Resources` section the expansion produced.
    resources: Map<String, Value>,
    /// Each declaration whose logical id the expansion CHANGED, against the id its
    /// resource now carries — what [`rename_references`] re-points the references to.
    renamed: BTreeMap<String, String>,
}

/// Whether the `Transform` section names the SAM macro, in any of the spellings the section
/// admits — read by the one reader of that section
/// ([`super::template::declared_transforms`]), so the two can never disagree about what a
/// template declares.
fn declares_the_transform(document: &Value) -> bool {
    declared_transforms(document.get("Transform"))
        .iter()
        .any(|name| name == TRANSFORM)
}

/// The `Resources` section the transform produces from the one the template declares, with
/// the logical ids it renamed on the way.
fn expanded_resources(resources: &Map<String, Value>, globals: &Value) -> Expansion {
    // The functions' properties, resolved against `Globals` once: the API events are read
    // off them, and so is each function's own expansion.
    let functions: Vec<(String, Value)> = resources
        .iter()
        .filter(|(_, declaration)| resource_type(declaration) == Some(FUNCTION))
        .map(|(logical_id, declaration)| {
            (
                logical_id.clone(),
                merged(globals.get("Function"), declaration.get("Properties")),
            )
        })
        .collect();
    let stages = api_stages(resources, globals);
    let events = api_events(&functions, &stages);

    // `primary` carries the resource each declaration BECOMES (and every resource the
    // template declares itself); `added` the ones the transform synthesizes beside it. They
    // are kept apart so a synthesized id can never displace a resource the template
    // declares under that id — the declaration wins, which is what merging `added` under
    // `or_insert` below does.
    let mut primary: Map<String, Value> = Map::new();
    let mut added: Map<String, Value> = Map::new();
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    for (logical_id, declaration) in resources {
        let condition = declaration.get("Condition");
        match resource_type(declaration) {
            Some(FUNCTION) => {
                let properties = functions
                    .iter()
                    .find(|(id, _)| id == logical_id)
                    .map(|(_, properties)| properties.clone())
                    .unwrap_or(Value::Null);
                expand_function(
                    &mut primary,
                    &mut added,
                    (logical_id, declaration, &properties),
                    &events,
                );
            }
            Some(API) => {
                let properties = merged(globals.get("Api"), declaration.get("Properties"));
                expand_rest_api(
                    &mut primary,
                    &mut added,
                    (logical_id, declaration, &properties),
                    &Map::new(),
                );
            }
            Some(HTTP_API) => {
                let properties = merged(globals.get("HttpApi"), declaration.get("Properties"));
                expand_http_api(
                    &mut primary,
                    &mut added,
                    (logical_id, declaration, &properties),
                    &Map::new(),
                );
            }
            Some(SIMPLE_TABLE) => {
                let properties = merged(globals.get("SimpleTable"), declaration.get("Properties"));
                primary.insert(
                    logical_id.clone(),
                    carried_over(
                        declaration,
                        "AWS::DynamoDB::Table",
                        expanded_table(&properties),
                    ),
                );
            }
            Some(LAYER_VERSION) => {
                let properties = object(declaration.get("Properties"));
                let expanded = expanded_layer_version(&properties);
                // "AWS SAM … appends a hash to the logical ID": a layer version cannot be
                // updated in place, so each content is its own resource — and every
                // reference to the declaration is re-pointed at the id that gives it
                // ([`rename_references`]).
                let hashed = format!("{logical_id}{}", digest(&Value::Object(expanded.clone())));
                let mut layer = carried_over(declaration, "AWS::Lambda::LayerVersion", expanded);
                retained(&mut layer, &properties);
                primary.insert(hashed.clone(), layer);
                if &hashed != logical_id {
                    renamed.insert(logical_id.clone(), hashed);
                }
            }
            Some(STATE_MACHINE) => {
                let properties = object(declaration.get("Properties"));
                let (expanded, role) = expanded_state_machine(logical_id, &properties);
                primary.insert(
                    logical_id.clone(),
                    carried_over(declaration, "AWS::StepFunctions::StateMachine", expanded),
                );
                if let Some(role) = role {
                    added.insert(format!("{logical_id}Role"), conditioned(role, condition));
                }
            }
            // A type the transform does not expand (an `AWS::Serverless::Application`,
            // whose template lives in the Serverless Application Repository) stays exactly
            // as declared, and so does every resource the template writes out itself.
            _ => {
                primary.insert(logical_id.clone(), declaration.clone());
            }
        }
    }

    // The implicit APIs: one REST API for every `Api` event that named no `RestApiId`, one
    // HTTP API for every `HttpApi` event that named no `ApiId`.
    implicit_api(&mut added, &events, true);
    implicit_api(&mut added, &events, false);

    let mut out = primary;
    for (logical_id, resource) in added {
        out.entry(logical_id).or_insert(resource);
    }
    Expansion {
        resources: out,
        renamed,
    }
}

/// One API event a function declares: what it contributes to its API's definition, and the
/// permission that lets API Gateway invoke the function.
#[derive(Debug, Clone)]
struct ApiEvent {
    /// The function the event triggers.
    function: String,
    /// The key the function's `Events` map carries the event under.
    event: String,
    /// `true` for an `Api` event (REST, `AWS::ApiGateway`), `false` for a `HttpApi` one.
    rest: bool,
    /// The logical id of the API the event is routed through.
    api: String,
    /// Whether that API is one the transform synthesizes.
    implicit: bool,
    /// `Path`, as the event states it.
    path: String,
    /// `Method`, lowercased; `any` for the event that answers every method.
    method: String,
    /// The stage the API is deployed to, where its name is known: what the permission's
    /// logical id ends with.
    stage: Option<String>,
}

impl ApiEvent {
    /// The logical id of the permission: the function's id, the event's key, `Permission`
    /// and the stage's name (`MyFunctionApiEventPermissionProd`).
    fn permission_id(&self) -> String {
        format!(
            "{}{}Permission{}",
            self.function,
            self.event,
            self.stage.as_deref().unwrap_or_default()
        )
    }
}

/// Every API event the template's functions declare, in the order the functions and their
/// `Events` maps are walked.
fn api_events(
    functions: &[(String, Value)],
    stages: &Map<String, Value>,
) -> Vec<ApiEvent> {
    let mut events = Vec::new();
    for (function, properties) in functions {
        for (event, declaration) in object(properties.get("Events")) {
            let kind = declaration.get("Type").and_then(Value::as_str);
            let rest = match kind {
                Some("Api") => true,
                Some("HttpApi") => false,
                _ => continue,
            };
            let event_properties = object(declaration.get("Properties"));
            // "Path … Method" are what a route IS; an event stating neither routes nothing,
            // and an `HttpApi` event may state `Path`/`Method` or neither (it then answers
            // every route of the API, which is not a route this expansion can write).
            let (Some(path), Some(method)) = (
                event_properties.get("Path").and_then(Value::as_str),
                event_properties.get("Method").and_then(Value::as_str),
            ) else {
                continue;
            };
            let declared_api = event_properties
                .get(if rest { "RestApiId" } else { "ApiId" })
                .and_then(referenced_logical_id);
            let implicit = declared_api.is_none();
            let api = declared_api.unwrap_or_else(|| {
                if rest {
                    IMPLICIT_REST_API.to_string()
                } else {
                    IMPLICIT_HTTP_API.to_string()
                }
            });
            let stage = if implicit && rest {
                Some(IMPLICIT_STAGE.to_string())
            } else if implicit {
                None
            } else {
                stages.get(&api).and_then(Value::as_str).map(logical_fragment)
            };
            events.push(ApiEvent {
                function: function.clone(),
                event: event.clone(),
                rest,
                api,
                implicit,
                path: path.to_string(),
                method: method.to_ascii_lowercase(),
                stage,
            });
        }
    }
    events
}

/// Each explicitly declared API's stage name, as the declaration states it: the name a
/// stage's logical id and an API permission's logical id are built from.
fn api_stages(resources: &Map<String, Value>, globals: &Value) -> Map<String, Value> {
    let mut stages = Map::new();
    for (logical_id, declaration) in resources {
        let section = match resource_type(declaration) {
            Some(API) => "Api",
            Some(HTTP_API) => "HttpApi",
            _ => continue,
        };
        let properties = merged(globals.get(section), declaration.get("Properties"));
        if let Some(name) = properties.get("StageName") {
            stages.insert(logical_id.clone(), name.clone());
        }
    }
    stages
}

/// Expand one `AWS::Serverless::Function`: the Lambda function, the role it executes as,
/// and one resource per event source it declares.
fn expand_function(
    primary: &mut Map<String, Value>,
    added: &mut Map<String, Value>,
    declared: (&String, &Value, &Value),
    events: &[ApiEvent],
) {
    let (logical_id, declaration, properties) = declared;
    let condition = declaration.get("Condition");
    let mut function = Map::new();
    if let Some(code) = function_code(properties) {
        function.insert("Code".to_string(), code);
    }
    copy_through(&mut function, properties, FUNCTION_PASSTHROUGH);
    // "Tracing: … the function's X-Ray tracing mode", which Lambda spells as a
    // `TracingConfig$Mode`.
    if let Some(mode) = properties.get("Tracing") {
        function.insert("TracingConfig".to_string(), json!({ "Mode": mode }));
    }
    // "DeadLetterQueue: … TargetArn: the ARN of the queue or topic", which Lambda spells as
    // a `DeadLetterConfig$TargetArn`.
    if let Some(target) = properties.get("DeadLetterQueue").and_then(|queue| queue.get("TargetArn"))
    {
        function.insert(
            "DeadLetterConfig".to_string(),
            json!({ "TargetArn": target }),
        );
    }
    if let Some(uri) = properties.get("ImageUri") {
        function.insert("Code".to_string(), json!({ "ImageUri": uri }));
        function.insert("PackageType".to_string(), json!("Image"));
    }
    function
        .entry("Timeout".to_string())
        .or_insert_with(|| json!(DEFAULT_TIMEOUT));
    function.insert(
        "Tags".to_string(),
        tag_list(properties.get("Tags"), Some(CREATED_BY_TAG)),
    );
    // "Role: … If this property isn't specified, AWS SAM creates an IAM role for you."
    match properties.get("Role") {
        Some(role) => {
            function.insert("Role".to_string(), role.clone());
        }
        None => {
            function.insert(
                "Role".to_string(),
                json!({ "Fn::GetAtt": [format!("{logical_id}Role"), "Arn"] }),
            );
            added.insert(
                format!("{logical_id}Role"),
                conditioned(execution_role(logical_id, properties), condition),
            );
        }
    }
    primary.insert(
        logical_id.clone(),
        carried_over(declaration, "AWS::Lambda::Function", function),
    );

    for event in events.iter().filter(|event| &event.function == logical_id) {
        added.insert(
            event.permission_id(),
            conditioned(invoke_permission(event, logical_id), condition),
        );
    }
    for (event, declaration) in object(properties.get("Events")) {
        let Some(kind) = declaration.get("Type").and_then(Value::as_str) else {
            continue;
        };
        let event_properties = object(declaration.get("Properties"));
        let logical = format!("{logical_id}{event}");
        if let Some((_, source)) = EVENT_SOURCE_MAPPINGS
            .iter()
            .find(|(event_type, _)| *event_type == kind)
        {
            let Some(mapping) = event_source_mapping(logical_id, source, &event_properties) else {
                continue;
            };
            added.insert(
                logical,
                conditioned(
                    resource("AWS::Lambda::EventSourceMapping", mapping),
                    condition,
                ),
            );
            continue;
        }
        match kind {
            "Schedule" | "ScheduleV2" | "CloudWatchEvent" | "EventBridgeRule" => {
                let Some(rule) = event_rule(logical_id, &logical, &event_properties) else {
                    continue;
                };
                added.insert(
                    logical.clone(),
                    conditioned(resource("AWS::Events::Rule", rule), condition),
                );
                added.insert(
                    format!("{logical}Permission"),
                    conditioned(
                        resource(
                            "AWS::Lambda::Permission",
                            principal_permission(
                                logical_id,
                                "events.amazonaws.com",
                                json!({ "Fn::GetAtt": [logical.clone(), "Arn"] }),
                            ),
                        ),
                        condition,
                    ),
                );
            }
            "SNS" => {
                let Some(topic) = event_properties.get("Topic") else {
                    continue;
                };
                let mut subscription = Map::new();
                subscription.insert("Protocol".to_string(), json!("lambda"));
                subscription.insert("TopicArn".to_string(), topic.clone());
                subscription.insert(
                    "Endpoint".to_string(),
                    json!({ "Fn::GetAtt": [logical_id.clone(), "Arn"] }),
                );
                copy_through(
                    &mut subscription,
                    &Value::Object(event_properties.clone()),
                    &["FilterPolicy", "FilterPolicyScope", "RedrivePolicy", "Region"],
                );
                added.insert(
                    logical.clone(),
                    conditioned(resource("AWS::SNS::Subscription", subscription), condition),
                );
                added.insert(
                    format!("{logical}Permission"),
                    conditioned(
                        resource(
                            "AWS::Lambda::Permission",
                            principal_permission(logical_id, "sns.amazonaws.com", topic.clone()),
                        ),
                        condition,
                    ),
                );
            }
            // Every other event type (an S3 notification, a Cognito trigger) rewrites a
            // resource beside the function rather than adding one of its own; nothing is
            // synthesized for it here.
            _ => {}
        }
    }
}

/// The `Code` a function's declaration states: "CodeUri: The Amazon S3 URI … or a local
/// path", "InlineCode: the function code itself".
fn function_code(properties: &Value) -> Option<Value> {
    if let Some(code) = properties.get("InlineCode") {
        return Some(json!({ "ZipFile": code }));
    }
    lambda_s3_location(properties.get("CodeUri")?)
}

/// The execution role the transform creates for a function that names none: the role Lambda
/// assumes, carrying the basic execution managed policy, the managed policies the
/// declaration names, the one each of its event sources needs and one inline policy per
/// other `Policies` entry.
fn execution_role(logical_id: &str, properties: &Value) -> Value {
    let (declared_managed, inline) = role_policies(logical_id, properties);
    let mut managed = vec![json!(arn(BASIC_EXECUTION_POLICY))];
    managed.extend(declared_managed);
    for policy in event_source_policies(properties) {
        // A policy is either attached or it is not, so two events needing the same one
        // attach it once.
        if !managed.contains(&policy) {
            managed.push(policy);
        }
    }
    let mut role = Map::new();
    role.insert(
        "AssumeRolePolicyDocument".to_string(),
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Action": ["sts:AssumeRole"],
                "Effect": "Allow",
                "Principal": { "Service": ["lambda.amazonaws.com"] },
            }],
        }),
    );
    role.insert("ManagedPolicyArns".to_string(), Value::Array(managed));
    if !inline.is_empty() {
        role.insert("Policies".to_string(), Value::Array(inline));
    }
    if let Some(boundary) = properties.get("PermissionsBoundary") {
        role.insert("PermissionsBoundary".to_string(), boundary.clone());
    }
    role.insert(
        "Tags".to_string(),
        tag_list(properties.get("Tags"), Some(CREATED_BY_TAG)),
    );
    resource("AWS::IAM::Role", role)
}

/// The managed policies the function's own EVENT SOURCES need on the role it executes as,
/// one per source type that publishes one ([`EVENT_SOURCE_POLICIES`]), in the order the
/// `Events` entries are walked and without repeats.
fn event_source_policies(properties: &Value) -> Vec<Value> {
    let mut policies: Vec<Value> = Vec::new();
    for declaration in object(properties.get("Events")).values() {
        let Some(kind) = declaration.get("Type").and_then(Value::as_str) else {
            continue;
        };
        let Some((_, policy)) = EVENT_SOURCE_POLICIES
            .iter()
            .find(|(event_type, _)| *event_type == kind)
        else {
            continue;
        };
        let policy = json!(arn(policy));
        if !policies.contains(&policy) {
            policies.push(policy);
        }
    }
    policies
}

/// `Policies` as the list of entries it states, whichever of its spellings the declaration
/// uses: "This property accepts a single value or a list of values."
fn declared_policies(policies: Option<&Value>) -> Vec<Value> {
    match policies {
        Some(Value::Array(entries)) => entries.clone(),
        Some(entry) => vec![entry.clone()],
        None => Vec::new(),
    }
}

/// What a `Policies` declaration puts on the role the transform generates for it: the
/// managed policy ARNs to attach, and the INLINE policies to embed.
///
/// Each entry is one of the three things the property accepts, and every entry that is not
/// a managed policy becomes exactly ONE inline policy, named for the resource the role
/// belongs to and the entry's POSITION in the declared list
/// (`<LogicalId>RolePolicy<position>`), so the role embeds the entries in declaration
/// order:
///
/// * "a list of … AWS managed policy names, or … policy ARNs" — a string names a policy to
///   attach, which is a `ManagedPolicyArns` entry and no inline policy at all;
/// * an IAM policy document, which `Statement` is what makes a value one — embedded as it
///   is stated;
/// * "a list of AWS SAM policy templates" — a single-member object naming a published
///   template and the parameters it is applied with, embedded as the document that template
///   grants ([`policy_templates`]).
///
/// An entry that is none of those three (an object that is no document and names no
/// template this module carries) contributes no inline policy, rather than a document
/// nothing computed.
fn role_policies(logical_id: &str, properties: &Value) -> (Vec<Value>, Vec<Value>) {
    let mut managed: Vec<Value> = Vec::new();
    let mut inline: Vec<Value> = Vec::new();
    for (position, policy) in declared_policies(properties.get("Policies"))
        .iter()
        .enumerate()
    {
        let document = match policy {
            Value::String(_) => {
                managed.push(policy.clone());
                continue;
            }
            Value::Object(_) if policy.get("Statement").is_some() => Some(policy.clone()),
            Value::Object(entry) => named_policy_template(entry)
                .and_then(|(name, parameters)| policy_templates::document(name, parameters)),
            _ => None,
        };
        if let Some(document) = document {
            inline.push(json!({
                "PolicyName": format!("{logical_id}RolePolicy{position}"),
                "PolicyDocument": document,
            }));
        }
    }
    (managed, inline)
}

/// The template a policy-template entry names and the parameters it states for it: a
/// policy template is applied as a ONE-member object, the member's name being the
/// template's and its value the parameters.
fn named_policy_template(entry: &Map<String, Value>) -> Option<(&str, &Value)> {
    match entry.len() {
        1 => entry
            .iter()
            .next()
            .map(|(name, parameters)| (name.as_str(), parameters)),
        _ => None,
    }
}

/// The permission that lets API Gateway invoke the function this event routes to.
fn invoke_permission(event: &ApiEvent, function: &str) -> Value {
    let mut permission = Map::new();
    permission.insert("Action".to_string(), json!("lambda:InvokeFunction"));
    permission.insert(
        "FunctionName".to_string(),
        json!({ "Fn::GetAtt": [function, "Arn"] }),
    );
    permission.insert("Principal".to_string(), json!("apigateway.amazonaws.com"));
    // "SourceArn: … the ARN of the API's method", with the stage left open (`*`) so the
    // permission holds for the deployment the stage points at now and after a redeploy.
    let method = if event.method == "any" {
        "*".to_string()
    } else {
        event.method.to_ascii_uppercase()
    };
    let path = event
        .path
        .trim_start_matches('/')
        .split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                "*"
            } else {
                segment
            }
        })
        .collect::<Vec<&str>>()
        .join("/");
    permission.insert(
        "SourceArn".to_string(),
        json!({ "Fn::Sub": [
            arn(&format!(
                "execute-api:${{AWS::Region}}:${{AWS::AccountId}}:\
                 ${{__ApiId__}}/${{__Stage__}}/{method}/{path}"
            )),
            { "__ApiId__": { "Ref": event.api.clone() }, "__Stage__": "*" },
        ]}),
    );
    resource("AWS::Lambda::Permission", permission)
}

/// The permission that lets another AWS service invoke the function.
fn principal_permission(function: &str, principal: &str, source: Value) -> Map<String, Value> {
    let mut permission = Map::new();
    permission.insert("Action".to_string(), json!("lambda:InvokeFunction"));
    permission.insert(
        "FunctionName".to_string(),
        json!({ "Fn::GetAtt": [function, "Arn"] }),
    );
    permission.insert("Principal".to_string(), json!(principal));
    permission.insert("SourceArn".to_string(), source);
    permission
}

/// The event-source mapping a stream, queue or broker event becomes.
fn event_source_mapping(
    function: &str,
    source: &str,
    event_properties: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    let mut mapping = Map::new();
    mapping.insert(
        "FunctionName".to_string(),
        json!({ "Fn::GetAtt": [function, "Arn"] }),
    );
    // The event states its source under its own name (`Queue`, `Stream`, `Broker`); the
    // mapping spells every one of them `EventSourceArn`.
    mapping.insert(
        "EventSourceArn".to_string(),
        event_properties.get(source)?.clone(),
    );
    copy_through(
        &mut mapping,
        &Value::Object(event_properties.clone()),
        EVENT_SOURCE_PASSTHROUGH,
    );
    Some(mapping)
}

/// The EventBridge rule a schedule or event-pattern event becomes, with the function as its
/// target.
fn event_rule(
    function: &str,
    logical_id: &str,
    event_properties: &Map<String, Value>,
) -> Option<Map<String, Value>> {
    let mut rule = Map::new();
    if let Some(schedule) = event_properties.get("Schedule") {
        rule.insert("ScheduleExpression".to_string(), schedule.clone());
    }
    if let Some(pattern) = event_properties.get("Pattern") {
        rule.insert("EventPattern".to_string(), pattern.clone());
    }
    if rule.is_empty() {
        // Neither a schedule nor a pattern: nothing states when the rule fires.
        return None;
    }
    copy_through(
        &mut rule,
        &Value::Object(event_properties.clone()),
        &["Description", "Name", "State", "EventBusName"],
    );
    rule.insert(
        "Targets".to_string(),
        json!([{
            "Arn": { "Fn::GetAtt": [function, "Arn"] },
            "Id": format!("{logical_id}LambdaTarget"),
        }]),
    );
    Some(rule)
}

/// Expand one `AWS::Serverless::Api` (or the implicit REST API) into the API, its
/// deployment and its stage.
fn expand_rest_api(
    primary: &mut Map<String, Value>,
    added: &mut Map<String, Value>,
    declared: (&String, &Value, &Value),
    paths: &Map<String, Value>,
) {
    let (logical_id, declaration, properties) = declared;
    let condition = declaration.get("Condition");
    let mut api = Map::new();
    copy_through(&mut api, properties, REST_API_PASSTHROUGH);
    // "DefinitionBody: The OpenAPI specification that describes your API", which
    // `AWS::ApiGateway::RestApi` spells `Body`; "DefinitionUri: The Amazon S3 URI … of the
    // OpenAPI document", which it spells `BodyS3Location`.
    if !paths.is_empty() {
        api.insert("Body".to_string(), rest_definition(paths));
    } else if let Some(body) = properties.get("DefinitionBody") {
        api.insert("Body".to_string(), body.clone());
    } else if let Some(uri) = properties.get("DefinitionUri").and_then(s3_location) {
        api.insert("BodyS3Location".to_string(), uri);
    }
    if let Some(tags) = properties.get("Tags") {
        api.insert("Tags".to_string(), tag_list(Some(tags), None));
    }
    let definition = api
        .get("Body")
        .or_else(|| api.get("BodyS3Location"))
        .cloned()
        .unwrap_or_else(|| Value::Object(api.clone()));
    primary.insert(
        logical_id.clone(),
        carried_over(declaration, "AWS::ApiGateway::RestApi", api),
    );

    // The deployment and the stage: "AWS SAM creates an `AWS::ApiGateway::Deployment` and an
    // `AWS::ApiGateway::Stage` for the API."
    let digest = digest(&definition);
    let deployment_id = format!("{logical_id}Deployment{digest}");
    added.insert(
        deployment_id.clone(),
        conditioned(
            resource(
                "AWS::ApiGateway::Deployment",
                Map::from_iter([
                    (
                        "Description".to_string(),
                        json!(format!("RestApi deployment id: {digest}")),
                    ),
                    ("RestApiId".to_string(), json!({ "Ref": logical_id })),
                ]),
            ),
            condition,
        ),
    );
    let stage_name = properties
        .get("StageName")
        .cloned()
        .unwrap_or_else(|| json!(IMPLICIT_STAGE));
    let mut stage = Map::new();
    copy_through(&mut stage, properties, REST_STAGE_PASSTHROUGH);
    stage.insert("RestApiId".to_string(), json!({ "Ref": logical_id }));
    stage.insert("DeploymentId".to_string(), json!({ "Ref": deployment_id }));
    stage.insert("StageName".to_string(), stage_name.clone());
    if let Some(tags) = properties.get("Tags") {
        stage.insert("Tags".to_string(), tag_list(Some(tags), None));
    }
    let suffix = stage_name.as_str().map(logical_fragment).unwrap_or_default();
    added.insert(
        format!("{logical_id}{suffix}Stage"),
        conditioned(resource("AWS::ApiGateway::Stage", stage), condition),
    );
}

/// Expand one `AWS::Serverless::HttpApi` (or the implicit HTTP API) into the API and its
/// stage. An HTTP API deploys itself (`AutoDeploy`), so there is no deployment resource.
fn expand_http_api(
    primary: &mut Map<String, Value>,
    added: &mut Map<String, Value>,
    declared: (&String, &Value, &Value),
    routes: &Map<String, Value>,
) {
    let (logical_id, declaration, properties) = declared;
    let condition = declaration.get("Condition");
    let mut api = Map::new();
    if !routes.is_empty() {
        api.insert("Body".to_string(), http_definition(routes));
    } else if let Some(body) = properties.get("DefinitionBody") {
        api.insert("Body".to_string(), body.clone());
    } else if let Some(uri) = properties.get("DefinitionUri").and_then(s3_location) {
        api.insert("BodyS3Location".to_string(), uri);
    }
    copy_through(&mut api, properties, &["Description", "Name", "FailOnWarnings"]);
    if let Some(tags) = properties.get("Tags") {
        api.insert("Tags".to_string(), tags.clone());
    }
    primary.insert(
        logical_id.clone(),
        carried_over(declaration, "AWS::ApiGatewayV2::Api", api),
    );

    let stage_name = properties
        .get("StageName")
        .cloned()
        .unwrap_or_else(|| json!(HTTP_DEFAULT_STAGE));
    let mut stage = Map::new();
    stage.insert("ApiId".to_string(), json!({ "Ref": logical_id }));
    stage.insert("StageName".to_string(), stage_name.clone());
    stage.insert("AutoDeploy".to_string(), json!(true));
    copy_through(
        &mut stage,
        properties,
        &[
            "AccessLogSettings",
            "DefaultRouteSettings",
            "Description",
            "RouteSettings",
            "StageVariables",
        ],
    );
    if let Some(tags) = properties.get("Tags") {
        stage.insert("Tags".to_string(), tags.clone());
    }
    // The `$default` stage's name is not a logical-id fragment, so its stage carries the
    // suffix AWS SAM gives the default stage.
    let suffix = match stage_name.as_str() {
        Some(HTTP_DEFAULT_STAGE) | None => HTTP_DEFAULT_STAGE_SUFFIX.to_string(),
        Some(name) => format!("{}Stage", logical_fragment(name)),
    };
    added.insert(
        format!("{logical_id}{suffix}"),
        conditioned(resource("AWS::ApiGatewayV2::Stage", stage), condition),
    );
}

/// The implicit API the `Api` (or `HttpApi`) events that named no API of their own are
/// routed through, where the template has any.
fn implicit_api(added: &mut Map<String, Value>, events: &[ApiEvent], rest: bool) {
    let mut paths: Map<String, Value> = Map::new();
    for event in events
        .iter()
        .filter(|event| event.implicit && event.rest == rest)
    {
        let item = if rest {
            rest_path_item(&event.function)
        } else {
            http_path_item(&event.function)
        };
        let methods = paths
            .entry(event.path.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(methods) = methods.as_object_mut() {
            methods.insert(method_key(&event.method), item);
        }
    }
    if paths.is_empty() {
        return;
    }
    let logical_id = if rest {
        IMPLICIT_REST_API.to_string()
    } else {
        IMPLICIT_HTTP_API.to_string()
    };
    // The implicit API is declared by nothing, so it carries no resource attributes and no
    // properties of its own: its definition is the routes its events state.
    let declaration = Value::Object(Map::new());
    let properties = Value::Object(Map::new());
    let mut primary = Map::new();
    if rest {
        expand_rest_api(
            &mut primary,
            added,
            (&logical_id, &declaration, &properties),
            &paths,
        );
    } else {
        expand_http_api(
            &mut primary,
            added,
            (&logical_id, &declaration, &properties),
            &paths,
        );
    }
    for (id, resource) in primary {
        added.entry(id).or_insert(resource);
    }
}

/// The OpenAPI 2.0 document the implicit REST API's routes make up.
fn rest_definition(paths: &Map<String, Value>) -> Value {
    json!({
        "swagger": "2.0",
        "info": { "title": { "Fn::Sub": "${AWS::StackName}" }, "version": "1.0" },
        "paths": paths,
    })
}

/// The OpenAPI 3.0 document the implicit HTTP API's routes make up.
fn http_definition(paths: &Map<String, Value>) -> Value {
    json!({
        "openapi": "3.0.1",
        "info": { "title": { "Fn::Sub": "${AWS::StackName}" }, "version": "1.0" },
        "paths": paths,
    })
}

/// The key a method is spelled under in an OpenAPI path item: the method itself, or the
/// API Gateway extension that answers every method.
fn method_key(method: &str) -> String {
    if method == "any" {
        "x-amazon-apigateway-any-method".to_string()
    } else {
        method.to_string()
    }
}

/// One REST path item: the Lambda proxy integration that routes the method to the function.
fn rest_path_item(function: &str) -> Value {
    json!({
        "responses": {},
        "x-amazon-apigateway-integration": {
            "httpMethod": "POST",
            "type": "aws_proxy",
            "uri": { "Fn::Sub": lambda_integration_uri(function) },
        },
    })
}

/// One HTTP API path item: the same proxy integration, with the payload format version an
/// HTTP API's integration carries.
fn http_path_item(function: &str) -> Value {
    json!({
        "responses": {},
        "x-amazon-apigateway-integration": {
            "httpMethod": "POST",
            "type": "aws_proxy",
            "payloadFormatVersion": "2.0",
            "uri": { "Fn::Sub": lambda_integration_uri(function) },
        },
    })
}

/// The integration URI a Lambda proxy integration carries: the API Gateway path that
/// invokes the function, left as the `Fn::Sub` the expansion writes — the function's ARN is
/// not known until the function exists, so what the API's definition carries is the
/// expression for it.
fn lambda_integration_uri(function: &str) -> String {
    arn(&format!(
        "apigateway:${{AWS::Region}}:lambda:path/2015-03-31/functions/${{{function}.Arn}}/\
         invocations"
    ))
}

/// An ARN the expansion writes, in the partition this emulator's ARNs belong to.
///
/// The recorded expansion spells the partition out rather than leaving an
/// `${AWS::Partition}` for CloudFormation to substitute, so these ARNs do too; the
/// `${…}` expressions that remain are the ones whose value is not known until the
/// resources exist, which is why they are carried as an `Fn::Sub`.
fn arn(rest: &str) -> String {
    format!("arn:{PARTITION}:{rest}")
}

/// Expand one `AWS::Serverless::SimpleTable` into the DynamoDB table it declares: "a
/// DynamoDB table with a single attribute primary key".
fn expanded_table(properties: &Value) -> Map<String, Value> {
    let key = properties.get("PrimaryKey");
    // "PrimaryKey … Name: The name of the primary key. Default: id", "Type … Default:
    // String".
    let name = key
        .and_then(|key| key.get("Name"))
        .cloned()
        .unwrap_or_else(|| json!("id"));
    let declared_type = key
        .and_then(|key| key.get("Type"))
        .and_then(Value::as_str)
        .unwrap_or("String");
    let mut table = Map::new();
    table.insert(
        "KeySchema".to_string(),
        json!([{ "AttributeName": name, "KeyType": "HASH" }]),
    );
    table.insert(
        "AttributeDefinitions".to_string(),
        json!([{ "AttributeName": name, "AttributeType": attribute_type(declared_type) }]),
    );
    copy_through(&mut table, properties, TABLE_PASSTHROUGH);
    // "If you don't specify the ProvisionedThroughput property, the table is created with
    // on-demand capacity."
    if !table.contains_key("ProvisionedThroughput") {
        table.insert("BillingMode".to_string(), json!("PAY_PER_REQUEST"));
    }
    if let Some(tags) = properties.get("Tags") {
        table.insert("Tags".to_string(), tag_list(Some(tags), None));
    }
    table
}

/// The DynamoDB attribute type a `PrimaryKey$Type` names: "String", "Number" and "Binary"
/// are what the property admits, and DynamoDB's own `AttributeDefinition$AttributeType`
/// spells them `S`, `N` and `B`. Anything else is passed through as the declaration spells
/// it, for DynamoDB to judge.
fn attribute_type(declared: &str) -> &str {
    match declared {
        "String" => "S",
        "Number" => "N",
        "Binary" => "B",
        other => other,
    }
}

/// Expand one `AWS::Serverless::LayerVersion` into the Lambda layer version it declares.
fn expanded_layer_version(properties: &Map<String, Value>) -> Map<String, Value> {
    let mut layer = Map::new();
    // `AWS::Lambda::LayerVersion$Content` is the deployment package, whose members AWS
    // Lambda spells the way `PublishLayerVersion`'s own `Content` spells them
    // ([`lambda_s3_location`]) — not the `Bucket`/`Key`/`Version` of the `S3Location` an
    // API definition carries.
    if let Some(content) = properties.get("ContentUri").and_then(lambda_s3_location) {
        layer.insert("Content".to_string(), content);
    }
    copy_through(
        &mut layer,
        &Value::Object(properties.clone()),
        &[
            "CompatibleArchitectures",
            "CompatibleRuntimes",
            "Description",
            "LicenseInfo",
        ],
    );
    if let Some(name) = properties.get("LayerName") {
        layer.insert("LayerName".to_string(), name.clone());
    }
    layer
}

/// State the generated layer version's `DeletionPolicy` from the declaration's
/// [`RETENTION_POLICY_PROPERTY`], whose default is to retain the version.
///
/// The hashed logical id above makes each content a resource of its own, so a changed
/// content REMOVES the resource the stack had — the event `DeletionPolicy` governs — and
/// retaining the old version is exactly what the declaration's retention policy asks for.
/// A policy the declaration states itself wins: [`carried_over`] has already put it on the
/// resource, and this states one only where the declaration left the attribute out.
fn retained(layer: &mut Value, properties: &Map<String, Value>) {
    let Some(members) = layer.as_object_mut() else {
        return;
    };
    if members.contains_key(DELETION_POLICY) {
        return;
    }
    let policy = match properties
        .get(RETENTION_POLICY_PROPERTY)
        .and_then(Value::as_str)
    {
        Some(stated) if stated.eq_ignore_ascii_case(super::deletion_policy::DELETE) => {
            super::deletion_policy::DELETE
        }
        _ => super::deletion_policy::RETAIN,
    };
    members.insert(DELETION_POLICY.to_string(), json!(policy));
}

/// Re-point every reference to a logical id the expansion RENAMED at the id that resource
/// now carries.
///
/// A macro may rename what it expands — a layer version's logical id carries a content hash
/// — and CloudFormation plans and deploys the macro's OUTPUT, so it is the OUTPUT's
/// references that have to resolve: a `Ref` to the declaration's own id would otherwise name
/// a resource the processed template no longer declares. Each of the four ways a template
/// names a resource is re-pointed: `Ref`, `Fn::GetAtt` (both spellings), an `Fn::Sub`
/// variable and the `DependsOn` attribute. A name no expansion renamed is left exactly as
/// the template wrote it.
///
/// Bounded by [`MAX_TEMPLATE_DEPTH`], like every other walk over a caller-supplied document.
fn rename_references(node: &mut Value, renamed: &BTreeMap<String, String>, depth: usize) {
    if renamed.is_empty() || depth > MAX_TEMPLATE_DEPTH {
        return;
    }
    match node {
        Value::Array(items) => {
            for item in items.iter_mut() {
                rename_references(item, renamed, depth + 1);
            }
        }
        Value::Object(members) => {
            for (key, member) in members.iter_mut() {
                match key.as_str() {
                    // "Ref returns the value of the specified … resource", and "DependsOn
                    // … specif[ies] that the creation of a specific resource follows
                    // another": both name a resource by its logical id alone.
                    REF | DEPENDS_ON => rename_named(member, renamed),
                    // "Fn::GetAtt returns the value of an attribute from a resource in the
                    // template": the resource is the first argument, or everything before
                    // the first dot of the short form.
                    super::template::FN_GET_ATT => rename_attribute(member, renamed),
                    // "Fn::Sub substitutes variables in an input string": a `${Name}` names
                    // what a `Ref` names and a `${Name.Attribute}` what an `Fn::GetAtt`
                    // does.
                    FN_SUB => rename_substitution(member, renamed),
                    _ => {}
                }
                rename_references(member, renamed, depth + 1);
            }
        }
        _ => {}
    }
}

/// Re-point a reference that names a resource by its logical id alone — one name, or a list
/// of them, which is the form `DependsOn` also takes.
fn rename_named(member: &mut Value, renamed: &BTreeMap<String, String>) {
    match member {
        Value::String(name) => {
            if let Some(expanded) = renamed.get(name.as_str()) {
                *name = expanded.clone();
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                rename_named(item, renamed);
            }
        }
        _ => {}
    }
}

/// Re-point the resource an `Fn::GetAtt` reads its attribute off, in both spellings the
/// function takes: `[logicalId, attribute]` and `logicalId.attribute`.
fn rename_attribute(member: &mut Value, renamed: &BTreeMap<String, String>) {
    match member {
        Value::Array(parts) => {
            if let Some(Value::String(name)) = parts.first_mut() {
                if let Some(expanded) = renamed.get(name.as_str()) {
                    *name = expanded.clone();
                }
            }
        }
        Value::String(path) => {
            // The resource is read the way the crate's one reader of the short form reads
            // it, so the two can never disagree about which resource a path names.
            let resource = super::template::resource_of(path).to_string();
            if let Some(expanded) = renamed.get(resource.as_str()) {
                let repointed = format!("{expanded}{}", &path[resource.len()..]);
                *path = repointed;
            }
        }
        _ => {}
    }
}

/// Re-point the `${Name}` and `${Name.Attribute}` variables of an `Fn::Sub`, in both forms
/// the function takes: the source string on its own, or `[ String, { Name: Value, … } ]`.
fn rename_substitution(member: &mut Value, renamed: &BTreeMap<String, String>) {
    // A variable the substitution supplies ITSELF is not a reference to a resource at all,
    // so a name the map carries is left exactly as written.
    let supplied: Vec<String> = match &*member {
        Value::Array(parts) => parts
            .get(1)
            .and_then(Value::as_object)
            .map(|variables| variables.keys().cloned().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let source = match member {
        Value::String(source) => source,
        Value::Array(parts) => match parts.first_mut() {
            Some(Value::String(source)) => source,
            _ => return,
        },
        _ => return,
    };
    let repointed = renamed_substitution(source, renamed, &supplied);
    *source = repointed;
}

/// One `Fn::Sub` source string with every variable naming a renamed resource re-pointed.
///
/// The variables are read the way the crate's one reader of them reads them
/// ([`super::template::substitution_names`]): everything between `${` and the next `}`, the
/// name being what precedes the first dot. A `${!Literal}` is the function's own escape and
/// names nothing, so it never matches a renamed id; every other run of the string is copied
/// through byte for byte.
fn renamed_substitution(
    source: &str,
    renamed: &BTreeMap<String, String>,
    supplied: &[String],
) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start + 2]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(after);
            return out;
        };
        let variable = &after[..end];
        let trimmed = variable.trim();
        let name = trimmed.split('.').next().unwrap_or(trimmed);
        match renamed.get(name) {
            Some(expanded) if !supplied.iter().any(|variable| variable == name) => {
                // The attribute path the variable read the name off is kept as it was.
                out.push_str(expanded);
                out.push_str(&trimmed[name.len()..]);
            }
            _ => out.push_str(variable),
        }
        out.push('}');
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Expand one `AWS::Serverless::StateMachine` into the state machine it declares, and the
/// role it executes as where the declaration names none.
fn expanded_state_machine(
    logical_id: &str,
    properties: &Map<String, Value>,
) -> (Map<String, Value>, Option<Value>) {
    let mut machine = Map::new();
    // "Definition: The state machine definition", which Step Functions takes as a
    // `DefinitionString`; "DefinitionUri: The Amazon S3 URI … of the state machine
    // definition", which it takes as a `DefinitionS3Location`.
    if let Some(definition) = properties.get("Definition") {
        machine.insert(
            "DefinitionString".to_string(),
            json!({ "Fn::Sub": canonical(definition, 0) }),
        );
    } else if let Some(uri) = properties.get("DefinitionUri").and_then(s3_location) {
        machine.insert("DefinitionS3Location".to_string(), uri);
    }
    if let Some(name) = properties.get("Name") {
        machine.insert("StateMachineName".to_string(), name.clone());
    }
    if let Some(kind) = properties.get("Type") {
        machine.insert("StateMachineType".to_string(), kind.clone());
    }
    if let Some(logging) = properties.get("Logging") {
        machine.insert("LoggingConfiguration".to_string(), logging.clone());
    }
    if let Some(tracing) = properties.get("Tracing") {
        machine.insert("TracingConfiguration".to_string(), tracing.clone());
    }
    copy_through(
        &mut machine,
        &Value::Object(properties.clone()),
        &["DefinitionSubstitutions"],
    );
    if let Some(tags) = properties.get("Tags") {
        machine.insert("Tags".to_string(), tag_list(Some(tags), None));
    }
    match properties.get("Role") {
        Some(role) => {
            machine.insert("RoleArn".to_string(), role.clone());
            (machine, None)
        }
        None => {
            machine.insert(
                "RoleArn".to_string(),
                json!({ "Fn::GetAtt": [format!("{logical_id}Role"), "Arn"] }),
            );
            let mut role = Map::new();
            role.insert(
                "AssumeRolePolicyDocument".to_string(),
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Action": ["sts:AssumeRole"],
                        "Effect": "Allow",
                        "Principal": { "Service": ["states.amazonaws.com"] },
                    }],
                }),
            );
            // A state machine states its permissions the same way a function does, and the
            // role generated for it carries them the same way ([`role_policies`]).
            let (managed, inline) =
                role_policies(logical_id, &Value::Object(properties.clone()));
            if !managed.is_empty() {
                role.insert("ManagedPolicyArns".to_string(), Value::Array(managed));
            }
            if !inline.is_empty() {
                role.insert("Policies".to_string(), Value::Array(inline));
            }
            (machine, Some(resource("AWS::IAM::Role", role)))
        }
    }
}

/// One synthesized resource: its type and its properties.
fn resource(resource_type: &str, properties: Map<String, Value>) -> Value {
    json!({ "Type": resource_type, "Properties": properties })
}

/// A synthesized resource under the same `Condition` as the declaration it came from: "you
/// can … create resources conditionally", and a resource the transform adds for a
/// declaration exists exactly when that declaration's resource does.
fn conditioned(mut resource: Value, condition: Option<&Value>) -> Value {
    if let (Some(condition), Some(members)) = (condition, resource.as_object_mut()) {
        members.insert("Condition".to_string(), condition.clone());
    }
    resource
}

/// The resource a declaration BECOMES: its new type and properties, carrying over the
/// resource ATTRIBUTES beside `Properties` that CloudFormation itself reads
/// (`DependsOn`, `Condition`, the policies and `Metadata`), because those are stated about
/// the resource the declaration stands for.
fn carried_over(declaration: &Value, resource_type: &str, properties: Map<String, Value>) -> Value {
    let mut out = Map::new();
    out.insert("Type".to_string(), json!(resource_type));
    out.insert("Properties".to_string(), Value::Object(properties));
    for attribute in [
        "Condition",
        "DeletionPolicy",
        "DependsOn",
        "Metadata",
        "UpdateReplacePolicy",
    ] {
        if let Some(value) = declaration.get(attribute) {
            out.insert(attribute.to_string(), value.clone());
        }
    }
    Value::Object(out)
}

/// Copy the members `names` lists from `properties` into `into`, where the declaration
/// states them: the members two types spell the same way.
fn copy_through(into: &mut Map<String, Value>, properties: &Value, names: &[&str]) {
    for name in names {
        if let Some(value) = properties.get(*name) {
            into.insert((*name).to_string(), value.clone());
        }
    }
}

/// A SAM `Tags` map as the `[{Key, Value}]` list a CloudFormation resource takes, with the
/// tag the transform adds where there is one.
///
/// A declaration that already states its tags as a LIST is left as it is: that is the form
/// the target resource takes.
fn tag_list(tags: Option<&Value>, extra: Option<(&str, &str)>) -> Value {
    let mut list: Vec<Value> = match tags {
        Some(Value::Array(entries)) => entries.clone(),
        Some(Value::Object(entries)) => entries
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
        _ => Vec::new(),
    };
    if let Some((key, value)) = extra {
        list.push(json!({ "Key": key, "Value": value }));
    }
    Value::Array(list)
}

/// The `{Bucket, Key, Version}` location an `s3://` URI or an object naming those members
/// states, as a resource's S3-location property takes it.
fn s3_location(value: &Value) -> Option<Value> {
    let (bucket, key, version) = s3_parts(value)?;
    let mut location = Map::new();
    location.insert("Bucket".to_string(), bucket);
    location.insert("Key".to_string(), key);
    if let Some(version) = version {
        location.insert("Version".to_string(), version);
    }
    Some(Value::Object(location))
}

/// The Amazon S3 location a LAMBDA package property states: a function's `Code` and a layer
/// version's `Content`, whose members AWS Lambda's own calls spell `S3Bucket`, `S3Key` and
/// `S3ObjectVersion` — against the `Bucket`, `Key` and `Version` of the `S3Location` an API
/// definition and a state machine definition carry ([`s3_location`]).
fn lambda_s3_location(value: &Value) -> Option<Value> {
    let (bucket, key, version) = s3_parts(value)?;
    let mut location = Map::new();
    location.insert("S3Bucket".to_string(), bucket);
    location.insert("S3Key".to_string(), key);
    if let Some(version) = version {
        location.insert("S3ObjectVersion".to_string(), version);
    }
    Some(Value::Object(location))
}

/// The bucket, key and object version a SAM URI property names: "`s3://bucket/key`" (with
/// an optional `?versionId=`), or an object stating `Bucket`, `Key` and `Version`.
///
/// `None` for anything else — a local path, which is what `sam package` replaces with an S3
/// URI before the template is deployed, is not a location this expansion can state. The URI
/// spelling is read by the crate's one reader of it
/// ([`super::template::s3_uri_parts`]).
fn s3_parts(value: &Value) -> Option<(Value, Value, Option<Value>)> {
    match value {
        Value::String(uri) => {
            let (bucket, key, version) = super::template::s3_uri_parts(uri)?;
            Some((json!(bucket), json!(key), version.map(|version| json!(version))))
        }
        Value::Object(members) => Some((
            members.get("Bucket")?.clone(),
            members.get("Key")?.clone(),
            members.get("Version").cloned(),
        )),
        _ => None,
    }
}

/// The logical id a `{"Ref": "Api"}` (or a plain logical id) names, which is how an event
/// states the API it is routed through.
fn referenced_logical_id(value: &Value) -> Option<String> {
    match value {
        Value::String(id) => Some(id.clone()),
        Value::Object(_) => value
            .get("Ref")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        _ => None,
    }
}

/// A name as a logical-id fragment: "the logical ID must be alphanumeric (A-Za-z0-9) and
/// unique within the template", so only the alphanumeric characters of a stage's name can
/// be part of one.
fn logical_fragment(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// The resource type a declaration states.
fn resource_type(declaration: &Value) -> Option<&str> {
    declaration.get("Type").and_then(Value::as_str)
}

/// A value's members, or an empty map when it has none — the one reading of an optional
/// object section (`Properties`, `Events`).
fn object(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// A resource's properties resolved against the `Globals` section: "properties that you
/// specify in the Globals section … apply to all your serverless functions and APIs", and
/// "the resource-level property overrides the global".
///
/// Two objects are merged member by member, so a resource stating one environment variable
/// keeps the global ones beside it; anything else the resource states REPLACES the global
/// value.
fn merged(global: Option<&Value>, declared: Option<&Value>) -> Value {
    match (global, declared) {
        (Some(Value::Object(global)), Some(Value::Object(declared))) => {
            let mut out = global.clone();
            for (name, value) in declared {
                match out.get(name) {
                    Some(global) if global.is_object() && value.is_object() => {
                        out.insert(name.clone(), merged(Some(global), Some(value)));
                    }
                    _ => {
                        out.insert(name.clone(), value.clone());
                    }
                }
            }
            Value::Object(out)
        }
        (_, Some(declared)) => declared.clone(),
        (Some(global), None) => global.clone(),
        (None, None) => Value::Object(Map::new()),
    }
}

/// The hash a deployment's (or a layer version's) logical id carries: [`DIGEST_CHARS`]
/// lowercase hex characters of a digest of the value's CONTENT.
///
/// The same content always gives the same id and different content a different one, which
/// is the whole of what the id has to do: it makes an immutable resource's identity a
/// function of what it holds, so an edited API definition is planned as a NEW deployment
/// rather than an update of the one that exists. Nothing reads the hash itself, so the
/// digest is the crate's own derivation ([`super::template::content_digest`]) rather than a
/// published one.
fn digest(value: &Value) -> String {
    super::template::content_digest(DIGEST_SALT, &canonical(value, 0), DIGEST_CHARS)
}

/// A value as text that depends on its CONTENT alone: object members in sorted order, so
/// the same definition hashes the same way whatever order the template reader built its
/// members in (`serde_json`'s map preserves insertion order under one build feature and
/// sorts under another).
///
/// Bounded by [`MAX_TEMPLATE_DEPTH`]: the value is caller-supplied, so a deeper tree is
/// reported as elided rather than recursed into.
fn canonical(value: &Value, depth: usize) -> String {
    if depth > MAX_TEMPLATE_DEPTH {
        return "...".to_string();
    }
    match value {
        Value::Object(members) => {
            let mut names: Vec<&String> = members.keys().collect();
            names.sort();
            let members: Vec<String> = names
                .iter()
                .map(|name| {
                    format!(
                        "{}:{}",
                        Value::String((*name).clone()),
                        canonical(&members[name.as_str()], depth + 1)
                    )
                })
                .collect();
            format!("{{{}}}", members.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items
                .iter()
                .map(|item| canonical(item, depth + 1))
                .collect();
            format!("[{}]", items.join(","))
        }
        scalar => scalar.to_string(),
    }
}
