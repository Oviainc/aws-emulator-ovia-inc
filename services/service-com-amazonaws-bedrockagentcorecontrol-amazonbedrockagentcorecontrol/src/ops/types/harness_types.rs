// SPDX-License-Identifier: CC0-1.0

//! The shapes the harness operations (`CreateHarness`, `GetHarness`, `UpdateHarness`,
//! `DeleteHarness`) reach.
//!
//! HAND-AUTHORED, not generated. The scaffold's op-stub generator declined all four
//! harness operations ("shape missing from model: smithy.api#Unit", recorded in
//! `src/ops/SKIPPED_STUBS.md`): the `Harness` closure reaches
//! `HarnessGatewayOutboundAuth`, a union two of whose members target `smithy.api#Unit`,
//! a prelude shape the model JSON does not carry and the vendored codegen cannot type.
//! Everything that union does not touch is declared with the ordinary derive here; the
//! union itself carries the one hand-written codec, below.
//!
//! Member sets, member order, requiredness and targets come from
//! `smithy_query(query="shape", shape="<Shape>", model_path="services/\
//! service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/\
//! smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json")`,
//! and the derive re-checks each one against that model at compile time.
//!
//! [`Harness`] itself gets its schema and codec from [`agentcore_schema_and_codec!`]
//! rather than the derive, because its `createdAt` / `updatedAt` target `DateTimestamp`
//! (`@timestampFormat("date-time")`) — see `types/date_time_wire.rs` for that whole story.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json");

// ---------------------------------------------------------------------------
// The harness resource
// ---------------------------------------------------------------------------

/// `Harness` — what every harness operation reports about one harness.
///
/// The member list below is the model's, member for member: same wire names, same order,
/// same shape types, same requiredness.
#[derive(Debug, Clone)]
pub struct Harness {
    pub harness_id: String,
    pub harness_name: String,
    pub arn: String,
    pub status: String,
    pub harness_version: Option<String>,
    pub execution_role_arn: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub model: HarnessModelConfiguration,
    pub system_prompt: Vec<HarnessSystemContentBlock>,
    pub tools: Vec<HarnessTool>,
    pub skills: Vec<HarnessSkill>,
    pub allowed_tools: Vec<String>,
    pub truncation: HarnessTruncationConfiguration,
    pub environment: HarnessEnvironmentProvider,
    pub environment_artifact: Option<HarnessEnvironmentArtifact>,
    pub environment_variables: Option<std::collections::HashMap<String, String>>,
    pub authorizer_configuration: Option<AuthorizerConfiguration>,
    pub memory: Option<HarnessMemoryConfiguration>,
    pub hooks: Option<Vec<HarnessHook>>,
    pub max_iterations: Option<i32>,
    pub max_tokens: Option<i32>,
    pub timeout_seconds: Option<i32>,
    pub failure_reason: Option<String>,
}

agentcore_schema_and_codec!(
    Harness,
    HARNESS_SCHEMA,
    "com.amazonaws.bedrockagentcorecontrol#Harness",
    "com.amazonaws.bedrockagentcorecontrol",
    "Harness",
    (M_HARNESS_ID, harness_id, "harnessId", 0, req, string),
    (M_HARNESS_NAME, harness_name, "harnessName", 1, req, string),
    (M_ARN, arn, "arn", 2, req, string),
    (M_STATUS, status, "status", 3, req, string),
    (M_HARNESS_VERSION, harness_version, "harnessVersion", 4, opt, string),
    (
        M_EXECUTION_ROLE_ARN,
        execution_role_arn,
        "executionRoleArn",
        5,
        req,
        string
    ),
    (M_CREATED_AT, created_at, "createdAt", 6, req, date_time),
    (M_UPDATED_AT, updated_at, "updatedAt", 7, req, date_time),
    (M_MODEL, model, "model", 8, req, union_of(HarnessModelConfiguration)),
    (
        M_SYSTEM_PROMPT,
        system_prompt,
        "systemPrompt",
        9,
        req,
        list_of_union(HarnessSystemContentBlock)
    ),
    (M_TOOLS, tools, "tools", 10, req, list_of_struct(HarnessTool)),
    (M_SKILLS, skills, "skills", 11, req, list_of_union(HarnessSkill)),
    (M_ALLOWED_TOOLS, allowed_tools, "allowedTools", 12, req, list_of_string),
    (
        M_TRUNCATION,
        truncation,
        "truncation",
        13,
        req,
        struct_of(HarnessTruncationConfiguration)
    ),
    (
        M_ENVIRONMENT,
        environment,
        "environment",
        14,
        req,
        union_of(HarnessEnvironmentProvider)
    ),
    (
        M_ENVIRONMENT_ARTIFACT,
        environment_artifact,
        "environmentArtifact",
        15,
        opt,
        union_of(HarnessEnvironmentArtifact)
    ),
    (
        M_ENVIRONMENT_VARIABLES,
        environment_variables,
        "environmentVariables",
        16,
        opt,
        map_of_string
    ),
    (
        M_AUTHORIZER_CONFIGURATION,
        authorizer_configuration,
        "authorizerConfiguration",
        17,
        opt,
        union_of(AuthorizerConfiguration)
    ),
    (
        M_MEMORY,
        memory,
        "memory",
        18,
        opt,
        union_of(HarnessMemoryConfiguration)
    ),
    (M_HOOKS, hooks, "hooks", 19, opt, list_of_union(HarnessHook)),
    (M_MAX_ITERATIONS, max_iterations, "maxIterations", 20, opt, integer),
    (M_MAX_TOKENS, max_tokens, "maxTokens", 21, opt, integer),
    (M_TIMEOUT_SECONDS, timeout_seconds, "timeoutSeconds", 22, opt, integer),
    (M_FAILURE_REASON, failure_reason, "failureReason", 23, opt, string),
);

// ---------------------------------------------------------------------------
// Model configuration
// ---------------------------------------------------------------------------

#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessModelConfiguration")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessModelConfiguration {
    bedrockModelConfig(HarnessBedrockModelConfig),
    openAiModelConfig(HarnessOpenAiModelConfig),
    geminiModelConfig(HarnessGeminiModelConfig),
    liteLlmModelConfig(HarnessLiteLlmModelConfig),
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessBedrockModelConfig")]
#[derive(Debug, Clone, Default)]
pub struct HarnessBedrockModelConfig {
    pub model_id: String,
    pub max_tokens: Option<i32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub api_format: Option<String>,
    pub additional_params: Option<::aws_smithy_types::Document>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessOpenAiModelConfig")]
#[derive(Debug, Clone, Default)]
pub struct HarnessOpenAiModelConfig {
    pub model_id: String,
    pub api_key_arn: String,
    pub api_base: Option<String>,
    pub max_tokens: Option<i32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub api_format: Option<String>,
    pub additional_params: Option<::aws_smithy_types::Document>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessGeminiModelConfig")]
#[derive(Debug, Clone, Default)]
pub struct HarnessGeminiModelConfig {
    pub model_id: String,
    pub api_key_arn: String,
    pub max_tokens: Option<i32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<i32>,
    pub additional_params: Option<::aws_smithy_types::Document>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessLiteLlmModelConfig")]
#[derive(Debug, Clone, Default)]
pub struct HarnessLiteLlmModelConfig {
    pub model_id: String,
    pub api_key_arn: Option<String>,
    pub api_base: Option<String>,
    pub max_tokens: Option<i32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub additional_params: Option<::aws_smithy_types::Document>,
}

// ---------------------------------------------------------------------------
// System prompt, tools and skills
// ---------------------------------------------------------------------------

#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessSystemContentBlock")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessSystemContentBlock {
    Text(String),
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessTool")]
#[derive(Debug, Clone, Default)]
pub struct HarnessTool {
    pub r#type: String,
    pub name: Option<String>,
    pub config: Option<HarnessToolConfiguration>,
}

#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessToolConfiguration")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessToolConfiguration {
    remoteMcp(HarnessRemoteMcpConfig),
    agentCoreBrowser(HarnessAgentCoreBrowserConfig),
    agentCoreGateway(HarnessAgentCoreGatewayConfig),
    inlineFunction(HarnessInlineFunctionConfig),
    agentCoreCodeInterpreter(HarnessAgentCoreCodeInterpreterConfig),
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessRemoteMcpConfig")]
#[derive(Debug, Clone, Default)]
pub struct HarnessRemoteMcpConfig {
    pub url: String,
    pub headers: Option<std::collections::HashMap<String, String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessAgentCoreBrowserConfig")]
#[derive(Debug, Clone, Default)]
pub struct HarnessAgentCoreBrowserConfig {
    pub browser_arn: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessAgentCoreGatewayConfig")]
#[derive(Debug, Clone, Default)]
pub struct HarnessAgentCoreGatewayConfig {
    pub gateway_arn: String,
    pub outbound_auth: Option<HarnessGatewayOutboundAuth>,
}

/// `smithy.api#Unit` — the target of a union member that carries no value, written on the
/// wire as the empty object `{}`.
///
/// Schema and codec are hand-written for the same reason the union below is: the model
/// JSON carries no `smithy.api#Unit` shape, so `#[smithy_macros::smithy(shape = ..)]`
/// refuses the id before it ever reaches its empty-struct path. This is what that path
/// emits for a shape with no members.
#[derive(Debug, Clone, Default)]
pub struct HarnessUnitValue {}

impl HarnessUnitValue {
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

impl ::aws_smithy_schema::serde::SerializableStruct for HarnessUnitValue {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for HarnessUnitValue {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        d.read_struct(&Self::SCHEMA, &mut |_, _| Ok(()))?;
        Ok(Self {})
    }
}

/// `HarnessGatewayOutboundAuth` — how a harness authenticates to an AgentCore Gateway.
///
/// HAND-WRITTEN schema and codec, the one place in this module the derive cannot be used:
/// `awsIam` and `none` target `smithy.api#Unit`, which the vendored codegen cannot
/// classify (it is a prelude shape, absent from the model's `shapes` map), so
/// `#[smithy_macros::smithy(union_for = ..)]` fails on this union alone. What follows is
/// exactly what that derive emits for a three-member union — one `M_<MEMBER>` member
/// schema per member in the model's member order, a `SCHEMA` of type `Union` naming them
/// in that order, the `<SHAPE>_SCHEMA` static and the `SerializableStruct` /
/// `DeserializableStruct` pair — with each valueless member written as an empty structure.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessGatewayOutboundAuth {
    /// SigV4-sign requests using the agent's execution role.
    awsIam(HarnessUnitValue),
    /// No authentication.
    None(HarnessUnitValue),
    /// OAuth credentials for outbound authentication to the gateway.
    Oauth(OAuthCredentialProvider),
}

#[allow(non_upper_case_globals)]
impl HarnessGatewayOutboundAuth {
    pub const M_AWS_IAM: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.bedrockagentcorecontrol#HarnessGatewayOutboundAuth$awsIam",
            "com.amazonaws.bedrockagentcorecontrol",
            "awsIam",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        "awsIam",
        0,
    );

    pub const M_NONE: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.bedrockagentcorecontrol#HarnessGatewayOutboundAuth$none",
            "com.amazonaws.bedrockagentcorecontrol",
            "none",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        "none",
        1,
    );

    pub const M_OAUTH: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.bedrockagentcorecontrol#HarnessGatewayOutboundAuth$oauth",
            "com.amazonaws.bedrockagentcorecontrol",
            "oauth",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        "oauth",
        2,
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.bedrockagentcorecontrol#HarnessGatewayOutboundAuth",
            "com.amazonaws.bedrockagentcorecontrol",
            "HarnessGatewayOutboundAuth",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[&Self::M_AWS_IAM, &Self::M_NONE, &Self::M_OAUTH],
    );
}

pub static HARNESS_GATEWAY_OUTBOUND_AUTH_SCHEMA: &::aws_smithy_schema::Schema =
    &HarnessGatewayOutboundAuth::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for HarnessGatewayOutboundAuth {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::awsIam(value) => s.write_struct(&Self::M_AWS_IAM, value)?,
            Self::None(value) => s.write_struct(&Self::M_NONE, value)?,
            Self::Oauth(value) => s.write_struct(&Self::M_OAUTH, value)?,
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for HarnessGatewayOutboundAuth {
    fn deserialize(
        d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut out: Option<Self> = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            // A union carries EXACTLY one member, so a second member on the wire is
            // ambiguous and is refused here rather than silently overwritten — the same
            // arity rule the derive enforces for every other union.
            let read = match member.member_index() {
                Some(0) => Some(Self::awsIam(
                    <HarnessUnitValue as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                )),
                Some(1) => Some(Self::None(
                    <HarnessUnitValue as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                )),
                Some(2) => Some(Self::Oauth(
                    <OAuthCredentialProvider as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                )),
                _ => None,
            };
            if let Some(read) = read {
                if out.is_some() {
                    return ::std::result::Result::Err(
                        ::aws_smithy_schema::serde::SerdeError::custom(
                            "union 'HarnessGatewayOutboundAuth' had more than one member set"
                                .to_string(),
                        ),
                    );
                }
                out = Some(read);
            }
            Ok(())
        })?;
        out.ok_or_else(|| {
            ::aws_smithy_schema::serde::SerdeError::custom(
                "union 'HarnessGatewayOutboundAuth' had no member set".to_string(),
            )
        })
    }
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessInlineFunctionConfig")]
#[derive(Debug, Clone)]
pub struct HarnessInlineFunctionConfig {
    pub description: String,
    pub input_schema: ::aws_smithy_types::Document,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#HarnessAgentCoreCodeInterpreterConfig"
)]
#[derive(Debug, Clone, Default)]
pub struct HarnessAgentCoreCodeInterpreterConfig {
    pub code_interpreter_arn: Option<String>,
}

#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessSkill")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessSkill {
    Path(String),
    S3(HarnessSkillS3Source),
    Git(HarnessSkillGitSource),
    awsSkills(HarnessSkillAwsSkillsSource),
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessSkillS3Source")]
#[derive(Debug, Clone, Default)]
pub struct HarnessSkillS3Source {
    pub uri: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessSkillGitSource")]
#[derive(Debug, Clone, Default)]
pub struct HarnessSkillGitSource {
    pub url: String,
    pub path: Option<String>,
    pub auth: Option<HarnessSkillGitAuth>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessSkillGitAuth")]
#[derive(Debug, Clone, Default)]
pub struct HarnessSkillGitAuth {
    pub credential_arn: String,
    pub username: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessSkillAwsSkillsSource")]
#[derive(Debug, Clone, Default)]
pub struct HarnessSkillAwsSkillsSource {
    pub paths: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Truncation
// ---------------------------------------------------------------------------

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessTruncationConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct HarnessTruncationConfiguration {
    pub strategy: String,
    pub config: Option<HarnessTruncationStrategyConfiguration>,
}

#[smithy_macros::smithy(
    union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessTruncationStrategyConfiguration"
)]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessTruncationStrategyConfiguration {
    slidingWindow(HarnessSlidingWindowConfiguration),
    Summarization(HarnessSummarizationConfiguration),
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#HarnessSlidingWindowConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct HarnessSlidingWindowConfiguration {
    pub messages_count: Option<i32>,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#HarnessSummarizationConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct HarnessSummarizationConfiguration {
    pub summary_ratio: Option<f32>,
    pub preserve_recent_messages: Option<i32>,
    pub summarization_system_prompt: Option<String>,
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessEnvironmentProvider")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessEnvironmentProvider {
    agentCoreRuntimeEnvironment(HarnessAgentCoreRuntimeEnvironment),
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#HarnessAgentCoreRuntimeEnvironment"
)]
#[derive(Debug, Clone, Default)]
pub struct HarnessAgentCoreRuntimeEnvironment {
    pub agent_runtime_arn: String,
    pub agent_runtime_name: String,
    pub agent_runtime_id: String,
    pub lifecycle_configuration: LifecycleConfiguration,
    pub network_configuration: NetworkConfiguration,
    pub filesystem_configurations: Option<Vec<FilesystemConfiguration>>,
}

#[smithy_macros::smithy(
    union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessEnvironmentProviderRequest"
)]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessEnvironmentProviderRequest {
    agentCoreRuntimeEnvironment(HarnessAgentCoreRuntimeEnvironmentRequest),
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#HarnessAgentCoreRuntimeEnvironmentRequest"
)]
#[derive(Debug, Clone, Default)]
pub struct HarnessAgentCoreRuntimeEnvironmentRequest {
    pub lifecycle_configuration: Option<LifecycleConfiguration>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub filesystem_configurations: Option<Vec<FilesystemConfiguration>>,
}

#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessEnvironmentArtifact")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessEnvironmentArtifact {
    containerConfiguration(ContainerConfiguration),
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessMemoryConfiguration")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessMemoryConfiguration {
    agentCoreMemoryConfiguration(HarnessAgentCoreMemoryConfiguration),
    managedMemoryConfiguration(HarnessManagedMemoryConfiguration),
    Disabled(HarnessDisabledMemoryConfiguration),
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#HarnessAgentCoreMemoryConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct HarnessAgentCoreMemoryConfiguration {
    pub arn: String,
    pub actor_id: Option<String>,
    pub messages_count: Option<i32>,
    pub retrieval_config:
        Option<std::collections::HashMap<String, HarnessAgentCoreMemoryRetrievalConfig>>,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#HarnessAgentCoreMemoryRetrievalConfig"
)]
#[derive(Debug, Clone, Default)]
pub struct HarnessAgentCoreMemoryRetrievalConfig {
    pub top_k: Option<i32>,
    pub relevance_score: Option<f32>,
    pub strategy_id: Option<String>,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#HarnessManagedMemoryConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct HarnessManagedMemoryConfiguration {
    pub arn: Option<String>,
    pub strategies: Option<Vec<String>>,
    pub event_expiry_duration: Option<i32>,
    pub encryption_key_arn: Option<String>,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#HarnessDisabledMemoryConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct HarnessDisabledMemoryConfiguration {}

// ---------------------------------------------------------------------------
// Lifecycle hooks
// ---------------------------------------------------------------------------

#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessHook")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessHook {
    beforeInvocation(HarnessBeforeInvocationHook),
    afterInvocation(HarnessAfterInvocationHook),
    beforeToolCall(HarnessBeforeToolCallHook),
    afterToolCall(HarnessAfterToolCallHook),
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessBeforeInvocationHook")]
#[derive(Debug, Clone)]
pub struct HarnessBeforeInvocationHook {
    pub name: String,
    pub target: HarnessHookTarget,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessAfterInvocationHook")]
#[derive(Debug, Clone)]
pub struct HarnessAfterInvocationHook {
    pub name: String,
    pub target: HarnessHookTarget,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessBeforeToolCallHook")]
#[derive(Debug, Clone)]
pub struct HarnessBeforeToolCallHook {
    pub name: String,
    pub target: HarnessHookTarget,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessAfterToolCallHook")]
#[derive(Debug, Clone)]
pub struct HarnessAfterToolCallHook {
    pub name: String,
    pub target: HarnessHookTarget,
}

#[smithy_macros::smithy(union_for = "com.amazonaws.bedrockagentcorecontrol#HarnessHookTarget")]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessHookTarget {
    Lambda(HarnessHookLambdaTarget),
    Sns(HarnessHookSnsTarget),
    eventBridge(HarnessHookEventBridgeTarget),
}

/// `timeoutSeconds` and `failureMode` carry `@default` (60 and `deny`), so the model makes
/// them non-optional with a fallback the codec supplies when the wire omits them.
#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessHookLambdaTarget")]
#[derive(Debug, Clone, Default)]
pub struct HarnessHookLambdaTarget {
    pub arn: String,
    pub timeout_seconds: i32,
    pub failure_mode: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessHookSnsTarget")]
#[derive(Debug, Clone, Default)]
pub struct HarnessHookSnsTarget {
    pub arn: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#HarnessHookEventBridgeTarget")]
#[derive(Debug, Clone, Default)]
pub struct HarnessHookEventBridgeTarget {
    pub arn: String,
}

// ---------------------------------------------------------------------------
// The operations' input and output shapes
// ---------------------------------------------------------------------------

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#CreateHarnessRequest")]
#[derive(Debug, Clone, Default)]
pub struct CreateHarnessInput {
    pub harness_name: String,
    pub client_token: Option<String>,
    pub execution_role_arn: String,
    pub environment: Option<HarnessEnvironmentProviderRequest>,
    pub environment_artifact: Option<HarnessEnvironmentArtifact>,
    pub environment_variables: Option<std::collections::HashMap<String, String>>,
    pub authorizer_configuration: Option<AuthorizerConfiguration>,
    pub model: Option<HarnessModelConfiguration>,
    pub system_prompt: Option<Vec<HarnessSystemContentBlock>>,
    pub tools: Option<Vec<HarnessTool>>,
    pub skills: Option<Vec<HarnessSkill>>,
    pub allowed_tools: Option<Vec<String>>,
    pub memory: Option<HarnessMemoryConfiguration>,
    pub truncation: Option<HarnessTruncationConfiguration>,
    pub hooks: Option<Vec<HarnessHook>>,
    pub max_iterations: Option<i32>,
    pub max_tokens: Option<i32>,
    pub timeout_seconds: Option<i32>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#CreateHarnessResponse")]
#[derive(Debug, Clone)]
pub struct CreateHarnessOutput {
    pub harness: Harness,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#GetHarnessRequest")]
#[derive(Debug, Clone, Default)]
pub struct GetHarnessInput {
    pub harness_id: String,
    pub harness_version: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#GetHarnessResponse")]
#[derive(Debug, Clone)]
pub struct GetHarnessOutput {
    pub harness: Harness,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#DeleteHarnessRequest")]
#[derive(Debug, Clone, Default)]
pub struct DeleteHarnessInput {
    pub harness_id: String,
    pub client_token: Option<String>,
    pub delete_managed_memory: Option<bool>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#DeleteHarnessResponse")]
#[derive(Debug, Clone, Default)]
pub struct DeleteHarnessOutput {
    pub harness: Option<Harness>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#UpdateHarnessRequest")]
#[derive(Debug, Clone, Default)]
pub struct UpdateHarnessInput {
    pub harness_id: String,
    pub client_token: Option<String>,
    pub execution_role_arn: Option<String>,
    pub environment: Option<HarnessEnvironmentProviderRequest>,
    pub environment_artifact: Option<UpdatedHarnessEnvironmentArtifact>,
    pub environment_variables: Option<std::collections::HashMap<String, String>>,
    pub authorizer_configuration: Option<UpdatedAuthorizerConfiguration>,
    pub model: Option<HarnessModelConfiguration>,
    pub system_prompt: Option<Vec<HarnessSystemContentBlock>>,
    pub tools: Option<Vec<HarnessTool>>,
    pub skills: Option<Vec<HarnessSkill>>,
    pub allowed_tools: Option<Vec<String>>,
    pub memory: Option<UpdatedHarnessMemoryConfiguration>,
    pub truncation: Option<HarnessTruncationConfiguration>,
    pub hooks: Option<Vec<HarnessHook>>,
    pub max_iterations: Option<i32>,
    pub max_tokens: Option<i32>,
    pub timeout_seconds: Option<i32>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#UpdateHarnessResponse")]
#[derive(Debug, Clone)]
pub struct UpdateHarnessOutput {
    pub harness: Harness,
}

/// The PATCH-semantics wrappers `UpdateHarness` takes: the wrapper present with
/// `optionalValue` sets the member, present without it clears the member, and absent
/// leaves it unchanged.
#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#UpdatedHarnessEnvironmentArtifact"
)]
#[derive(Debug, Clone, Default)]
pub struct UpdatedHarnessEnvironmentArtifact {
    pub optional_value: Option<HarnessEnvironmentArtifact>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.bedrockagentcorecontrol#UpdatedAuthorizerConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct UpdatedAuthorizerConfiguration {
    pub optional_value: Option<AuthorizerConfiguration>,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.bedrockagentcorecontrol#UpdatedHarnessMemoryConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct UpdatedHarnessMemoryConfiguration {
    pub optional_value: Option<HarnessMemoryConfiguration>,
}
