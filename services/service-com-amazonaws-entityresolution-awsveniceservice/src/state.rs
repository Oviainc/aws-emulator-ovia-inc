// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-entityresolution-awsveniceservice service.
//!
//! Four control-plane collections are persisted here, and every member of each
//! record is the MODELLED type of the member it is returned on, so a reader clones
//! what the writer stored and the two cannot drift: `CreateSchemaMapping` followed by
//! `GetSchemaMapping` returns the created record field-for-field.
//!
//! The provider-service collection — [`ProviderService`] — has no create API at all: a
//! provider
//! service is AWS-MANAGED catalog data, "the ProviderServices that are available in
//! this Amazon Web Services Region"
//! (`smithy_query(query="operation",operation="ListProviderServices")`), so the store
//! is SEEDED with the catalog when it is first touched (see [`provider_service_catalog`])
//! and no handler writes to it. `ListProviderServices` pages over exactly those
//! records and `GetProviderService` reads one of them, so the two cannot disagree.
//!
//! `hasWorkflows` is deliberately NOT a stored field. It is a property of the
//! relationship between the two maps — "is this schema mapping referenced by some
//! matching workflow's `inputSourceConfig`" — so storing it would let a workflow
//! create or delete leave a stale flag behind. It is derived on read by
//! [`ComAmazonawsEntityresolutionAwsveniceserviceState::schema_has_workflows`].

use std::collections::{BTreeMap, HashMap};

use ::aws_smithy_types::Document;
use sha2::{Digest, Sha256};

use crate::ops::types::{
    IdMappingIncrementalRunConfig, IdMappingTechniques, IdMappingWorkflowInputSource,
    IdMappingWorkflowOutputSource, IncrementalRunConfig, InputSource, OutputSource,
    ProviderComponentSchema, ProviderEndpointConfiguration, ProviderIdNameSpaceConfiguration,
    ProviderIntermediateDataAccessConfiguration, ProviderMarketplaceConfiguration,
    ProviderSchemaAttribute, ResolutionTechniques, SchemaInputAttribute,
};

/// One schema mapping as this service persists it.
///
/// `description` is `Option<String>` because the member is optional on the create
/// request and on `GetSchemaMapping`'s output; `CreateSchemaMapping`'s output models
/// it @required, which is served as the empty string when the create carried none.
#[derive(Debug, Clone)]
pub struct SchemaMapping {
    /// The name the schema mapping is keyed by, as the caller supplied it.
    pub schema_name: String,
    /// The ARN minted when the schema mapping was created.
    pub schema_arn: String,
    pub description: Option<String>,
    pub mapped_input_fields: Vec<SchemaInputAttribute>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub tags: Option<HashMap<String, String>>,
}

/// One matching workflow as this service persists it.
#[derive(Debug, Clone)]
pub struct MatchingWorkflow {
    /// The name the workflow is keyed by, as the caller supplied it.
    pub workflow_name: String,
    /// The ARN minted when the workflow was created.
    pub workflow_arn: String,
    pub description: Option<String>,
    pub input_source_config: Vec<InputSource>,
    pub output_source_config: Vec<OutputSource>,
    pub resolution_techniques: ResolutionTechniques,
    pub incremental_run_config: Option<IncrementalRunConfig>,
    pub role_arn: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub tags: Option<HashMap<String, String>>,
}

/// One provider service as this service persists it: an entry of the AWS-managed
/// catalog of "the ProviderServices that are available in this Amazon Web Services
/// Region".
///
/// There is no API that creates, updates or deletes one, so this record is written
/// exactly once — when the store is seeded by [`provider_service_catalog`] — and only
/// ever read after that, by `ListProviderServices` and `GetProviderService`.
///
/// Every member is the MODELLED type of the member it is returned on, EXCEPT the ARN,
/// which is not stored at all: `ProviderServiceArn` is region-scoped
/// (`^arn:…:entityresolution:<region>::providerservice/<providerName>/<providerServiceName>$`)
/// and carries no account, so it is a pure function of the entry plus the region the
/// request was signed for — see [`ProviderService::arn`]. Storing it would mean one
/// record per region.
#[derive(Debug, Clone)]
pub struct ProviderService {
    pub provider_name: String,
    pub provider_service_name: String,
    pub provider_service_display_name: String,
    /// A `ServiceType` value: `ASSIGNMENT` or `ID_MAPPING`.
    pub provider_service_type: String,
    pub anonymized_output: bool,
    pub provider_endpoint_configuration: ProviderEndpointConfiguration,
    pub provider_component_schema: ProviderComponentSchema,
    pub provider_configuration_definition: Option<Document>,
    pub provider_entity_output_definition: Document,
    pub provider_job_configuration: Option<Document>,
    pub provider_id_name_space_configuration: Option<ProviderIdNameSpaceConfiguration>,
    pub provider_intermediate_data_access_configuration:
        Option<ProviderIntermediateDataAccessConfiguration>,
}

impl ProviderService {
    /// The key this entry is stored and ordered under: `<providerName>/<providerServiceName>`.
    ///
    /// The pair is the catalog's identity — `GetProviderService` takes exactly those
    /// two — and the composite is also a legal `nextToken` (the modelled pattern
    /// `^[a-zA-Z_0-9-=+/]*$` admits `/`, and both halves are `EntityName`s).
    pub fn catalog_key(&self) -> String {
        format!("{}/{}", self.provider_name, self.provider_service_name)
    }

    /// The `providerServiceArn` for this entry in `region`.
    ///
    /// The modelled `ProviderServiceArn` pattern
    /// (`smithy_query(query="shape",shape="ProviderServiceSummary")`) fixes the whole
    /// shape of it: an EMPTY account segment, then
    /// `providerservice/<providerName>/<providerServiceName>`. It is therefore derived
    /// from the entry, never minted, so the ARN `ListProviderServices` reports is the
    /// one `GetProviderService` resolves.
    pub fn arn(&self, region: &str) -> String {
        format!(
            "arn:aws:entityresolution:{region}::providerservice/{}/{}",
            self.provider_name, self.provider_service_name
        )
    }
}

/// One ID mapping workflow as this service persists it.
///
/// `role_arn` is a plain `String` rather than an `Option<String>` because
/// `IdMappingRoleArn` is modelled with `default: ""`
/// (`smithy_query(query="operation",operation="CreateIdMappingWorkflow")`), so a
/// request that carries no role is the empty string and not a distinct "absent"
/// state. `output_source_config` and `incremental_run_config` have NO modelled
/// default, so absent stays absent for them.
#[derive(Debug, Clone)]
pub struct IdMappingWorkflow {
    /// The name the workflow is keyed by, as the caller supplied it.
    pub workflow_name: String,
    /// The ARN minted when the workflow was created.
    pub workflow_arn: String,
    pub description: Option<String>,
    pub input_source_config: Vec<IdMappingWorkflowInputSource>,
    pub output_source_config: Option<Vec<IdMappingWorkflowOutputSource>>,
    pub id_mapping_techniques: IdMappingTechniques,
    pub incremental_run_config: Option<IdMappingIncrementalRunConfig>,
    pub role_arn: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub tags: Option<HashMap<String, String>>,
}

/// Per-service state for com-amazonaws-entityresolution-awsveniceservice. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap`, not a `HashMap`, because the list operations are
/// `@paginated` and a page boundary is only meaningful over a stable order: the
/// `nextToken` a page returns is the key of the first record it did NOT return, so
/// the next call resumes at that key.
#[derive(Debug)]
pub struct ComAmazonawsEntityresolutionAwsveniceserviceState {
    /// Every schema mapping that exists, keyed by `schemaName`.
    pub schema_mappings: BTreeMap<String, SchemaMapping>,
    /// Every matching workflow that exists, keyed by `workflowName`.
    pub matching_workflows: BTreeMap<String, MatchingWorkflow>,
    /// The Region's provider-service catalog, keyed by
    /// [`ProviderService::catalog_key`]. Seeded, never written by a handler.
    pub provider_services: BTreeMap<String, ProviderService>,
    /// Every ID mapping workflow that exists, keyed by `workflowName`.
    ///
    /// A SEPARATE namespace from [`Self::matching_workflows`]: the two resources have
    /// their own ARN types (`matchingworkflow/…` vs `idmappingworkflow/…`), their own
    /// create/get/list/delete operations and their own URI prefixes, so the same name
    /// can name one of each.
    pub id_mapping_workflows: BTreeMap<String, IdMappingWorkflow>,
}

/// `Default` is what the engine builds the store with on first touch, so it is where
/// the provider-service catalog is seeded: the customer-owned collections start empty,
/// and the AWS-managed one starts complete.
impl Default for ComAmazonawsEntityresolutionAwsveniceserviceState {
    fn default() -> Self {
        Self {
            schema_mappings: BTreeMap::new(),
            matching_workflows: BTreeMap::new(),
            provider_services: provider_service_catalog(),
            id_mapping_workflows: BTreeMap::new(),
        }
    }
}

/// One row of the provider-service catalog table: everything that differs between
/// entries. The rest of a [`ProviderService`] is DERIVED from these fields by
/// [`provider_service_catalog`], so no two members of one entry can disagree.
struct CatalogRow {
    /// `providerName` — "typically the company name", an `EntityName`
    /// (`^[a-zA-Z_0-9-]*$`), so it carries no space or dot however the company spells
    /// its name; the spelled-out form belongs in `providerServiceDisplayName`.
    provider_name: &'static str,
    /// `providerServiceName` — "the name of the product that the provider service
    /// provides", also an `EntityName`.
    provider_service_name: &'static str,
    display_name: &'static str,
    /// A modelled `ServiceType`: `ASSIGNMENT` (the provider assigns its own id to each
    /// record of a matching workflow) or `ID_MAPPING` (it maps ids between two
    /// namespaces). Those two are the whole enum
    /// (`smithy_query(query="shape",shape="ServiceType")`).
    service_type: &'static str,
    /// Whether the provider's output is anonymized, i.e. cannot be related back to the
    /// identifying input (`GetProviderService$anonymizedOutput`).
    anonymized_output: bool,
    /// Whether this provider needs the caller to hand it an S3 bucket for intermediate
    /// data. `ProviderIntermediateDataAccessConfiguration` is documented as what "is
    /// required by SOME providers", so it is per-entry, and it is the same fact that
    /// decides whether the entry takes an intermediate-source configuration.
    intermediate_data_access: bool,
    /// The provider's input schema: `(fieldName, SchemaAttributeType, hashing)` per
    /// `providerSchemaAttributes` member.
    attributes: &'static [(&'static str, &'static str, bool)],
}

/// The provider-service catalog this emulator serves in every Region.
///
/// AWS's own catalog depends on the Data Exchange subscriptions of the account and on
/// the Region, so there is no single recorded answer to copy; what IS fixed by the
/// model is WHICH providers Entity Resolution integrates and which
/// `SchemaAttributeType`s each of them accepts, spelled out per provider in the doc of
/// `ProviderSchemaAttribute$type`
/// (`smithy_query(query="shape",shape="ProviderSchemaAttribute")`): LiveRamp,
/// TransUnion and Unified ID 2.0. This table is those three providers, with each
/// entry's attributes taken from that provider's own list in that doc, and it spans
/// both `ServiceType`s, both `anonymizedOutput` values, a provider with two services
/// and providers with one.
const CATALOG: &[CatalogRow] = &[
    CatalogRow {
        provider_name: "LiveRamp",
        provider_service_name: "IDENTITY_RESOLUTION",
        display_name: "LiveRamp Identity Resolution",
        service_type: "ASSIGNMENT",
        anonymized_output: false,
        intermediate_data_access: true,
        // LiveRamp's documented types, PII hashed where the provider takes a hash.
        attributes: &[
            ("Unique ID", "UNIQUE_ID", false),
            ("First Name", "NAME_FIRST", false),
            ("Last Name", "NAME_LAST", false),
            ("Address", "ADDRESS", false),
            ("City", "ADDRESS_CITY", false),
            ("State", "ADDRESS_STATE", false),
            ("Postal Code", "ADDRESS_POSTALCODE", false),
            ("Email", "EMAIL_ADDRESS", true),
            ("Phone", "PHONE_NUMBER", true),
        ],
    },
    CatalogRow {
        provider_name: "LiveRamp",
        provider_service_name: "TRANSCODE",
        display_name: "LiveRamp Transcode",
        service_type: "ID_MAPPING",
        anonymized_output: false,
        intermediate_data_access: true,
        // An id-mapping service takes ids, not PII: `UNIQUE_ID` in, `PROVIDER_ID` out,
        // both of which LiveRamp's documented list admits.
        attributes: &[
            ("Unique ID", "UNIQUE_ID", false),
            ("RampID", "PROVIDER_ID", false),
        ],
    },
    CatalogRow {
        provider_name: "TransUnion",
        provider_service_name: "IDENTITY_RESOLUTION",
        display_name: "TransUnion TruAudience Identity Resolution",
        service_type: "ASSIGNMENT",
        anonymized_output: false,
        intermediate_data_access: false,
        // TransUnion's documented types, which — unlike LiveRamp's — include `DATE`,
        // `IPV4` and `MAID`.
        attributes: &[
            ("Unique ID", "UNIQUE_ID", false),
            ("First Name", "NAME_FIRST", false),
            ("Last Name", "NAME_LAST", false),
            ("Email", "EMAIL_ADDRESS", true),
            ("Phone Number", "PHONE_NUMBER", true),
            ("Date of Birth", "DATE", false),
            ("IP Address", "IPV4", false),
            ("Device ID", "MAID", false),
        ],
    },
    CatalogRow {
        provider_name: "UnifiedId",
        provider_service_name: "IDENTITY_RESOLUTION",
        display_name: "Unified ID 2.0",
        service_type: "ID_MAPPING",
        // UID2 answers with tokens rather than with anything that identifies the
        // input record, which is exactly what `anonymizedOutput` reports.
        anonymized_output: true,
        intermediate_data_access: false,
        // Unified ID 2.0's documented list is the whole of its schema: three types.
        attributes: &[
            ("Unique ID", "UNIQUE_ID", false),
            ("Email", "EMAIL_ADDRESS", true),
            ("Phone Number", "PHONE_NUMBER", true),
        ],
    },
];

/// The catalog as the store holds it: [`CATALOG`] expanded into modelled records,
/// keyed and ordered by [`ProviderService::catalog_key`].
///
/// Every derived member is a function of the row, so the record is identical on every
/// boot: nothing here reads a clock, a request or a random source.
pub fn provider_service_catalog() -> BTreeMap<String, ProviderService> {
    CATALOG
        .iter()
        .map(|row| {
            let provider_service = ProviderService {
                provider_name: row.provider_name.to_string(),
                provider_service_name: row.provider_service_name.to_string(),
                provider_service_display_name: row.display_name.to_string(),
                provider_service_type: row.service_type.to_string(),
                anonymized_output: row.anonymized_output,
                provider_endpoint_configuration:
                    ProviderEndpointConfiguration::marketplaceConfiguration(
                        marketplace_configuration(row),
                    ),
                provider_component_schema: component_schema(row),
                provider_configuration_definition: configuration_definition(row),
                provider_entity_output_definition: entity_output_definition(row),
                // The optional `providerJobConfiguration` is "provider service job
                // configurations" — a provider-specific job document. This catalog
                // declares none, so the member is absent rather than guessed at.
                provider_job_configuration: None,
                provider_id_name_space_configuration: id_name_space_configuration(row),
                provider_intermediate_data_access_configuration:
                    intermediate_data_access_configuration(row),
            };
            (provider_service.catalog_key(), provider_service)
        })
        .collect()
}

/// The entry's Data Exchange identifiers (`ProviderMarketplaceConfiguration`).
///
/// AWS mints these four in Data Exchange, and this emulator has no Data Exchange, so
/// each one is [`data_exchange_id`] over the entry and the member name: a value in the
/// form a Data Exchange id takes, distinct per entry and per member, and identical on
/// every boot.
fn marketplace_configuration(row: &CatalogRow) -> ProviderMarketplaceConfiguration {
    ProviderMarketplaceConfiguration {
        data_set_id: data_exchange_id(row, "dataSetId"),
        revision_id: data_exchange_id(row, "revisionId"),
        asset_id: data_exchange_id(row, "assetId"),
        listing_id: data_exchange_id(row, "listingId"),
    }
}

/// A Data Exchange identifier for one member of one catalog entry: the first 16 bytes
/// of SHA-256 over `<providerName>/<providerServiceName>#<member>`, as 32 lowercase
/// hex characters — the form a Data Exchange data set, revision, asset or listing id
/// takes. Deterministic, so the id an entry reports never changes.
fn data_exchange_id(row: &CatalogRow, member: &str) -> String {
    let seed = format!(
        "{}/{}#{member}",
        row.provider_name, row.provider_service_name
    );
    hex::encode(&Sha256::digest(seed.as_bytes())[..16])
}

/// The entry's input schema (`ProviderComponentSchema`).
///
/// `providerSchemaAttributes` is the row's attribute list, and `schemas` — a list of
/// attribute-name lists — is the single schema those attributes make up, so the two
/// members cannot name different fields.
fn component_schema(row: &CatalogRow) -> ProviderComponentSchema {
    ProviderComponentSchema {
        schemas: Some(vec![row
            .attributes
            .iter()
            .map(|(field_name, _, _)| (*field_name).to_string())
            .collect()]),
        provider_schema_attributes: Some(
            row.attributes
                .iter()
                .map(|(field_name, attribute_type, hashing)| ProviderSchemaAttribute {
                    field_name: (*field_name).to_string(),
                    r#type: (*attribute_type).to_string(),
                    // `subType` is modelled optional and none of these attributes has
                    // one, so it is absent rather than echoed from `type`.
                    sub_type: None,
                    hashing: Some(*hashing),
                })
                .collect(),
        ),
    }
}

/// `providerConfigurationDefinition` — "the definition of the provider
/// configuration", i.e. the schema of the `providerConfiguration` document a workflow
/// hands this provider.
///
/// An entry that needs intermediate storage is exactly the one whose configuration
/// takes an intermediate S3 path, so the definition is derived from
/// [`CatalogRow::intermediate_data_access`]; an entry that takes no configuration has
/// no definition and the optional member is absent.
fn configuration_definition(row: &CatalogRow) -> Option<Document> {
    if !row.intermediate_data_access {
        return None;
    }
    Some(json_object([
        ("type", json_string("object")),
        (
            "properties",
            json_object([(
                "intermediateSourceConfiguration",
                json_object([
                    ("type", json_string("object")),
                    (
                        "properties",
                        json_object([(
                            "intermediateS3Path",
                            json_object([("type", json_string("string"))]),
                        )]),
                    ),
                    (
                        "required",
                        Document::Array(vec![json_string("intermediateS3Path")]),
                    ),
                ]),
            )]),
        ),
        (
            "required",
            Document::Array(vec![json_string("intermediateSourceConfiguration")]),
        ),
    ]))
}

/// `providerEntityOutputDefinition` — "the definition of the provider entity output".
///
/// Whatever the service type, the provider answers per input record with the record's
/// own `UNIQUE_ID` and the id the provider holds for it, so the definition is those
/// two fields, the second named for the provider and typed `PROVIDER_ID` (both are
/// modelled `SchemaAttributeType` values).
fn entity_output_definition(row: &CatalogRow) -> Document {
    Document::Object(HashMap::from([
        ("type".to_string(), json_string("object")),
        (
            "properties".to_string(),
            Document::Object(HashMap::from([
                (
                    "Unique ID".to_string(),
                    json_object([("type", json_string("UNIQUE_ID"))]),
                ),
                (
                    format!("{} ID", row.provider_name),
                    json_object([("type", json_string("PROVIDER_ID"))]),
                ),
            ])),
        ),
    ]))
}

/// `providerIdNameSpaceConfiguration` — "the provider configuration required for
/// different ID namespace types".
///
/// An ID namespace is a `SOURCE` or a `TARGET` of an ID-MAPPING workflow, so only an
/// `ID_MAPPING` entry has one; an `ASSIGNMENT` entry leaves the optional member absent.
fn id_name_space_configuration(row: &CatalogRow) -> Option<ProviderIdNameSpaceConfiguration> {
    if row.service_type != SERVICE_TYPE_ID_MAPPING {
        return None;
    }
    let definition = |role: &str| {
        Some(json_object([
            ("type", json_string("object")),
            (
                "properties",
                json_object([(
                    "idNamespaceRole",
                    json_object([("const", json_string(role))]),
                )]),
            ),
        ]))
    };
    Some(ProviderIdNameSpaceConfiguration {
        description: Some(format!(
            "{} ID namespace configuration",
            row.display_name
        )),
        provider_source_configuration_definition: definition("SOURCE"),
        provider_target_configuration_definition: definition("TARGET"),
    })
}

/// `providerIntermediateDataAccessConfiguration` — the accounts and S3 actions "that
/// are required by some providers to create an S3 bucket for intermediate data
/// storage", so it is present only for an entry that needs one.
///
/// The account is the emulator's own ([`::emulator_server::EMULATOR_ACCOUNT_ID`]),
/// which is the only account this emulator has and satisfies the modelled
/// `AwsAccountId` pattern `^\d{12}$`; the actions are the S3 operations a provider
/// performs on that bucket — it reads the input it was handed, writes its output and
/// lists the prefix.
fn intermediate_data_access_configuration(
    row: &CatalogRow,
) -> Option<ProviderIntermediateDataAccessConfiguration> {
    if !row.intermediate_data_access {
        return None;
    }
    Some(ProviderIntermediateDataAccessConfiguration {
        aws_account_ids: Some(vec![
            ::emulator_server::EMULATOR_ACCOUNT_ID.to_string()
        ]),
        required_bucket_actions: Some(vec![
            "s3:GetObject".to_string(),
            "s3:ListBucket".to_string(),
            "s3:PutObject".to_string(),
        ]),
    })
}

/// A `Document` object from `(name, value)` pairs.
fn json_object<'a>(members: impl IntoIterator<Item = (&'a str, Document)>) -> Document {
    Document::Object(
        members
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
    )
}

/// A `Document` string.
fn json_string(value: &str) -> Document {
    Document::String(value.to_string())
}

/// The `ServiceType` value for a service that maps ids between two ID namespaces.
pub const SERVICE_TYPE_ID_MAPPING: &str = "ID_MAPPING";

/// The text this service puts in a response `message` member.
///
/// Every message AWS Entity Resolution returns on these paths is the SERVICE'S OWN
/// per-operation text: a fixed string that names the resource TYPE the way the API
/// spells it (`SchemaMapping`, `MatchingWorkflow`), with the requested name
/// substituted only where that operation's own template includes it. None of it is a
/// sentence composed from the request or from the stored record, and none of it is
/// shared between operations: a missing workflow reads `MatchingWorkflow Not Found`
/// from `GetMatchingWorkflow` but `MatchingWorkflow <workflowName> does not exist`
/// from `DeleteMatchingWorkflow`, so collapsing the two into one string would be
/// wrong. Each path therefore gets its OWN item below, derived from no other.
pub mod messages {
    /// `DeleteSchemaMappingOutput$message` — modelled @required, target `String`, doc
    /// "A successful operation message"
    /// (`smithy_query(query="operation",operation="DeleteSchemaMapping")`). The value
    /// is this fixed service string, observed on the wire from AWS.
    pub const DELETE_SCHEMA_MAPPING_SUCCESS: &str = "SchemaMapping Deleted.";

    /// `DeleteMatchingWorkflowOutput$message` — modelled @required, target `String`,
    /// doc "A successful operation message". Not itself recorded; it is this
    /// operation's own constant, spelled with the resource type the API uses, in the
    /// same form as the recorded [`DELETE_SCHEMA_MAPPING_SUCCESS`].
    pub const DELETE_MATCHING_WORKFLOW_SUCCESS: &str = "MatchingWorkflow Deleted.";

    /// `GetSchemaMapping`'s `ResourceNotFoundException$message` (HTTP 404). AWS
    /// answers this constant and does NOT name the schema that was asked for.
    pub const GET_SCHEMA_MAPPING_NOT_FOUND: &str = "SchemaMapping does not exist";

    /// `GetMatchingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404). AWS
    /// answers this constant — deliberately worded differently from
    /// [`delete_matching_workflow_not_found`] — and does not name the workflow.
    pub const GET_MATCHING_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `GetProviderService`'s `ResourceNotFoundException$message` (HTTP 404), answered
    /// when no catalog entry has the requested `providerName` and
    /// `providerServiceArn`. Not itself recorded; it is this operation's own constant,
    /// in the same form as the recorded [`GET_SCHEMA_MAPPING_NOT_FOUND`] and spelled
    /// with the resource type the API uses, and like it, it does not name what was
    /// asked for.
    pub const GET_PROVIDER_SERVICE_NOT_FOUND: &str = "ProviderService does not exist";

    /// `CreateSchemaMapping`'s `ConflictException$message` (HTTP 400) when the
    /// `schemaName` is already taken. AWS answers this constant with no resource name
    /// in it; `ConflictException`'s own doc gives "Schema already exists" as the
    /// example of the condition.
    pub const CREATE_SCHEMA_MAPPING_CONFLICT: &str = "SchemaMapping already exists.";

    /// `CreateMatchingWorkflow`'s `ConflictException$message` (HTTP 400) when the
    /// `workflowName` is already taken. Not itself recorded; it is this operation's
    /// own constant, in the same form as the recorded
    /// [`CREATE_SCHEMA_MAPPING_CONFLICT`] and spelled with the resource type
    /// `ConflictException`'s doc names ("Workflow already exists").
    pub const CREATE_MATCHING_WORKFLOW_CONFLICT: &str = "MatchingWorkflow already exists.";

    /// `CreateIdMappingWorkflow`'s `ConflictException$message` (HTTP 400) when the
    /// `workflowName` is already taken — the refusal its own doc describes ("Each
    /// IdMappingWorkflow must have a unique workflow name"). Not itself recorded; it is
    /// this operation's own constant, in the same form as the recorded
    /// [`CREATE_SCHEMA_MAPPING_CONFLICT`] and spelled with the resource type the API
    /// uses for it (`IdMappingWorkflow`).
    pub const CREATE_ID_MAPPING_WORKFLOW_CONFLICT: &str = "IdMappingWorkflow already exists.";

    /// `GetIdMappingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404). Not
    /// itself recorded; this reader follows the same constant, no-name form as the
    /// recorded [`GET_SCHEMA_MAPPING_NOT_FOUND`], spelled with the resource type the
    /// API uses for it.
    pub const GET_ID_MAPPING_WORKFLOW_NOT_FOUND: &str = "IdMappingWorkflow Not Found";

    /// `DeleteSchemaMapping`'s `ConflictException$message` (HTTP 400) when a matching
    /// workflow still names the schema in its `inputSourceConfig` — the refusal this
    /// operation's own doc describes ("This operation will fail if there is a
    /// MatchingWorkflow object that references the SchemaMapping in the workflow's
    /// InputSourceConfig"). Not recorded; a constant, not a composed sentence.
    pub const DELETE_SCHEMA_MAPPING_IN_USE: &str =
        "SchemaMapping is referenced by the InputSourceConfig of a MatchingWorkflow";

    /// `DeleteMatchingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404).
    ///
    /// This is the one path where the service's text is a TEMPLATE over the request:
    /// the `workflowName` @httpLabel the caller asked for is substituted into it. The
    /// template is the rule — any name goes through it unchanged.
    pub fn delete_matching_workflow_not_found(workflow_name: &str) -> String {
        format!("MatchingWorkflow {workflow_name} does not exist")
    }

    /// `UpdateMatchingWorkflow`'s `ResourceNotFoundException$message` (HTTP 404) when
    /// no workflow carries the `workflowName` @httpLabel. Not itself recorded; it is
    /// this operation's own constant. `UpdateMatchingWorkflow` addresses the SAME
    /// resource by the SAME key as `GetMatchingWorkflow` ("The workflow must already
    /// exist for this operation to succeed"), and AWS's text for that lookup names no
    /// resource name, so this follows the recorded
    /// [`GET_MATCHING_WORKFLOW_NOT_FOUND`] form rather than the templated delete one.
    pub const UPDATE_MATCHING_WORKFLOW_NOT_FOUND: &str = "MatchingWorkflow Not Found";

    /// `DeleteSchemaMapping`'s `ResourceNotFoundException$message` (HTTP 404) — the
    /// refusal its doc describes ("This operation returns a ResourceNotFoundException
    /// if a schema with the given name does not exist"). Not itself recorded; this
    /// delete path follows the same request-templated form as the recorded
    /// [`delete_matching_workflow_not_found`], with the `schemaName` @httpLabel
    /// substituted.
    pub fn delete_schema_mapping_not_found(schema_name: &str) -> String {
        format!("SchemaMapping {schema_name} does not exist")
    }
}

/// How many schema mappings this emulator lets exist at once, past which
/// `CreateSchemaMapping` answers the modelled `ExceedsLimitException` (HTTP 402, "The
/// request was rejected because it attempted to create resources beyond the current
/// Entity Resolution account limits").
///
/// AWS Entity Resolution publishes an adjustable per-account, per-Region quota named
/// "Schema mappings"; this is the default this emulator enforces for it. The bound is
/// what keeps the store from growing without limit on repeated creates, and it is
/// self-healing: `DeleteSchemaMapping` releases an entry, so room comes back.
pub const MAX_SCHEMA_MAPPINGS: usize = 100;

/// The name of the quota [`MAX_SCHEMA_MAPPINGS`] enforces, reported on
/// `ExceedsLimitException$quotaName`.
pub const SCHEMA_MAPPINGS_QUOTA_NAME: &str = "Schema mappings";

/// How many matching workflows this emulator lets exist at once, past which
/// `CreateMatchingWorkflow` answers the modelled `ExceedsLimitException` (HTTP 402).
///
/// AWS Entity Resolution publishes an adjustable per-account, per-Region quota named
/// "Matching workflows"; this is the default this emulator enforces for it. As with
/// the schema-mapping bound, `DeleteMatchingWorkflow` releases an entry, so the bound
/// needs no TTL or eviction.
pub const MAX_MATCHING_WORKFLOWS: usize = 100;

/// The name of the quota [`MAX_MATCHING_WORKFLOWS`] enforces, reported on
/// `ExceedsLimitException$quotaName`.
pub const MATCHING_WORKFLOWS_QUOTA_NAME: &str = "Matching workflows";

/// How many ID mapping workflows this emulator lets exist at once, past which
/// `CreateIdMappingWorkflow` answers the modelled `ExceedsLimitException` (HTTP 402).
///
/// AWS Entity Resolution publishes an adjustable per-account, per-Region quota named
/// "ID mapping workflows"; this is the default this emulator enforces for it. As with
/// the matching-workflow bound, `DeleteIdMappingWorkflow` releases an entry, so the
/// bound needs no TTL or eviction.
pub const MAX_ID_MAPPING_WORKFLOWS: usize = 100;

/// The name of the quota [`MAX_ID_MAPPING_WORKFLOWS`] enforces, reported on
/// `ExceedsLimitException$quotaName`.
pub const ID_MAPPING_WORKFLOWS_QUOTA_NAME: &str = "ID mapping workflows";

/// A quota value as `ExceedsLimitException$quotaValue` models it (an `Integer`).
///
/// Every maximum is a small constant, so the conversion cannot fail; it is written
/// without a panic so a handler stays total.
pub fn quota_value(maximum: usize) -> Option<i32> {
    i32::try_from(maximum).ok()
}

/// The `ResolutionType` for which matching workflows have no incremental processing.
///
/// One of the values `ResolutionType` admits
/// (`RESOLUTION_TYPE_VALUES` = `ML_MATCHING | PROVIDER | RULE_MATCHING`); named here
/// because [`incremental_run_conflict`] is the one place the service branches on it.
pub const PROVIDER_RESOLUTION_TYPE: &str = "PROVIDER";

/// Why a matching-workflow request pairing an `incrementalRunConfig` with
/// `resolutionTechniques.resolutionType` `PROVIDER` is refused — `None` when the
/// combination is allowed.
///
/// Both `CreateMatchingWorkflow` and `UpdateMatchingWorkflow` document the same rule,
/// on the operation itself and on the member: "For workflows where `resolutionType`
/// is `PROVIDER`, incremental processing is not supported"
/// (`smithy_query(query="doc",shape="UpdateMatchingWorkflowInput",member="incrementalRunConfig")`).
/// The model cannot express a condition ACROSS two members, so the boundary validator
/// passes such a request and the handlers refuse it with the modelled
/// `ValidationException` (HTTP 400). The message is built from the request: it names
/// the member that cannot be carried and the `resolutionType` value the request
/// carried, which is what a caller needs to fix it.
pub fn incremental_run_conflict(
    resolution_type: &str,
    incremental_run_config: Option<&IncrementalRunConfig>,
) -> Option<String> {
    (incremental_run_config.is_some() && resolution_type == PROVIDER_RESOLUTION_TYPE).then(|| {
        format!(
            "incrementalRunConfig is not supported for a MatchingWorkflow whose \
             resolutionTechniques.resolutionType is {resolution_type}"
        )
    })
}

impl ComAmazonawsEntityresolutionAwsveniceserviceState {
    /// Is `schema_name` referenced by the `inputSourceConfig` of any stored matching
    /// workflow?
    ///
    /// `InputSource$schemaName` is the modelled reference from a workflow to a schema
    /// mapping ("The name of the schema to be retrieved",
    /// `smithy_query(query="shape",shape="InputSource")`), so this is what both
    /// `hasWorkflows` and `DeleteSchemaMapping`'s conflict check read.
    pub fn schema_has_workflows(&self, schema_name: &str) -> bool {
        self.matching_workflows.values().any(|workflow| {
            workflow
                .input_source_config
                .iter()
                .any(|source| source.schema_name == schema_name)
        })
    }
}

/// The region every ARN this service mints is scoped to when the request carries no
/// SigV4 credential scope to read it from. It is the region the emulator's own shared
/// AWS config names, so an unsigned call and a default-configured client agree.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`].
///
/// A resource ARN is region-scoped, and the only region a request carries is the one
/// in its SigV4 credential scope, so that is what an ARN minted while serving it uses.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// Now, truncated to milliseconds.
///
/// restJson1 writes a timestamp as epoch seconds with a fractional part, i.e. as a
/// JSON number: an `f64` holds today's epoch seconds to about a quarter of a
/// microsecond, so a nanosecond-precision instant would not survive the round trip
/// and `createdAt` would differ between the create response and a later read.
/// Milliseconds round-trip exactly, which keeps a stored timestamp stable across reads.
pub fn now() -> ::aws_smithy_types::DateTime {
    let millis = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The page size to serve for a `@paginated` operation whose `maxResults` member is
/// `max_results` and whose modelled `range` maximum is `modelled_max`.
///
/// `maxResults` is optional on both list operations, so an absent member is "the
/// caller set no bound", which is served with the modelled maximum rather than with
/// an empty page. The modelled `range` declares only a maximum, so zero and negative
/// values reach a handler too; they are read the same way, because a page size of
/// zero would answer an empty page with a continuation token forever. A positive
/// caller-supplied bound is already inside the modelled range by the time a handler
/// runs (the boundary validator refuses anything else); clamping again costs nothing
/// and keeps this total.
pub fn page_size(max_results: Option<i32>, modelled_max: usize) -> usize {
    match max_results.map(usize::try_from) {
        None | Some(Ok(0)) | Some(Err(_)) => modelled_max,
        Some(Ok(bound)) => bound.min(modelled_max),
    }
}

/// One page of an ordered `(key, record)` sequence, plus the `nextToken` to continue
/// from — `None` when the page reached the end.
///
/// The token is the KEY OF THE FIRST RECORD NOT RETURNED, and a resumed call starts
/// at the first key at or after the token. Resuming is therefore well defined even
/// when the record the token names was deleted between the two calls: the scan simply
/// continues at the next key.
///
/// A key is an `EntityName` (`^[a-zA-Z_0-9-]*$`, 1..255 characters), which is a strict
/// subset of the modelled `NextToken` (`^[a-zA-Z_0-9-=+/]*$`, 1..1024), so the token
/// this returns always satisfies the token member's own constraints.
pub fn page<T>(
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    let mut records = Vec::new();
    for (key, record) in ordered {
        if next_token.is_some_and(|token| key.as_str() < token) {
            continue;
        }
        if records.len() == page_size {
            return (records, Some(key));
        }
        records.push(record);
    }
    (records, None)
}

/// Could `value` be a `nextToken` this service issued?
///
/// Every token either list operation issues is a resource name, i.e. an `EntityName`:
/// `length 1..255, pattern ^[a-zA-Z_0-9-]*$`
/// (`smithy_query(query="operation",operation="ListMatchingWorkflows")`). The modelled
/// `NextToken` is wider (it also admits `=`, `+` and `/`), so a token the boundary
/// validator accepted can still be one this service never minted — refuse that rather
/// than serve an arbitrary suffix of the collection for it.
///
/// A token naming a record that has since been DELETED still satisfies this and is
/// still honoured — the scan resumes at the next name — so a page boundary survives a
/// concurrent delete, which is the behaviour a paging client depends on.
pub fn is_issued_page_token(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 255
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// Could `value` be a `nextToken` `ListProviderServices` issued?
///
/// A provider-service page token is a catalog key —
/// `<providerName>/<providerServiceName>`, two `EntityName`s — so it is checked as
/// exactly that. The modelled `NextToken` admits far more (`=`, `+`, `/` anywhere, up
/// to 1024 characters), so a token the boundary validator accepted can still be one
/// this service never minted: refuse that rather than serve an arbitrary suffix of the
/// catalog for it.
pub fn is_issued_provider_service_token(value: &str) -> bool {
    value.split_once('/').is_some_and(|(provider_name, provider_service_name)| {
        is_issued_page_token(provider_name) && is_issued_page_token(provider_service_name)
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEntityresolutionAwsveniceserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEntityresolutionAwsveniceserviceState>)
}
