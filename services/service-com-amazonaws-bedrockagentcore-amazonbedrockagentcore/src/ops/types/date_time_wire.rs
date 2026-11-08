// SPDX-License-Identifier: CC0-1.0

//! The schema and the serializer for a response shape with a
//! `@timestampFormat("date-time")` member.
//!
//! WHY THIS MODULE EXISTS. Two timestamp shapes reach this service's responses.
//! `smithy.api#Timestamp` carries no format, so a member targeting it renders in the
//! protocol default — epoch seconds, a JSON number — which is what the Memory event
//! surfaces report. `com.amazonaws.bedrockagentcore#DateTimestamp`, which every
//! `createdAt` of the tools surfaces targets, is marked `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="DateTimestamp",model_path="services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json")`),
//! so on this service's restJson1 wire such a member is an RFC 3339 / ISO-8601 STRING.
//! The JSON codec decides the rendering per member from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs`: `write_timestamp` reads
//! `Schema::timestamp_format()`), and falls back to the epoch number when the member
//! schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`, the HTTP
//! bindings and the JSON/XML names onto each member schema it emits, but it never emits
//! `.with_timestamp_format(..)`, so a `DateTimestamp` member built by the derive went
//! onto the wire as a bare epoch number — not even the modelled JSON type, so an SDK
//! deserializing it rejects or mis-parses the response. `vendor/` is read-only from
//! inside the emulator, so the shapes whose `createdAt` or `lastUpdatedAt` targets
//! `DateTimestamp` declare
//! their schema and serializer here instead, with the one chain call the codegen omits.
//! The sibling control-plane service compensates for the same gap in its own
//! `src/ops/types/date_time_wire.rs`; when the vendored codegen learns
//! `smithy.api#timestampFormat`, delete this module and put
//! `#[smithy_macros::smithy(shape = ...)]` back on the structures below.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@range`, …). The validating walk that enforces them
//!   runs on operation INPUTS only, and both shapes here are output-only.
//! * `with_list_member(..)`. Only the XML codecs read it, to name an element; this
//!   service's sole protocol is restJson1.
//! * `@jsonName`/`@xmlName`. No member below declares either: each one's wire name IS
//!   its Smithy member name.
//! * A `DeserializableStruct` impl. An operation's `Output` only has to be
//!   serializable (`smithy_server_runtime::Operation`), and nothing in this emulator
//!   reads a response back.
//!
//! Each member list below is the model's, member for member: same shape ids, same wire
//! names, same order (the member index is that position), same shape types.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::traits::TimestampFormat;
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};

use super::{
    GetBrowserSessionOutput, GetCodeInterpreterSessionOutput, StartBrowserSessionOutput,
    StartCodeInterpreterSessionOutput, StopCodeInterpreterSessionOutput,
};

/// The namespace every shape of this service lives in.
const NS: &str = "com.amazonaws.bedrockagentcore";

impl StartCodeInterpreterSessionOutput {
    pub const M_CODE_INTERPRETER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartCodeInterpreterSessionResponse$codeInterpreterIdentifier",
            NS,
            "codeInterpreterIdentifier",
        ),
        ShapeType::String,
        "codeInterpreterIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartCodeInterpreterSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartCodeInterpreterSessionResponse$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        2,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartCodeInterpreterSessionResponse",
            NS,
            "StartCodeInterpreterSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_CREATED_AT,
        ],
    );
}

pub static START_CODE_INTERPRETER_SESSION_OUTPUT_SCHEMA: &Schema =
    &StartCodeInterpreterSessionOutput::SCHEMA;

impl SerializableStruct for StartCodeInterpreterSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            self.code_interpreter_identifier.as_str(),
        )?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        Ok(())
    }
}

impl StopCodeInterpreterSessionOutput {
    pub const M_CODE_INTERPRETER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StopCodeInterpreterSessionResponse$codeInterpreterIdentifier",
            NS,
            "codeInterpreterIdentifier",
        ),
        ShapeType::String,
        "codeInterpreterIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StopCodeInterpreterSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_LAST_UPDATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StopCodeInterpreterSessionResponse$lastUpdatedAt",
            NS,
            "lastUpdatedAt",
        ),
        ShapeType::Timestamp,
        "lastUpdatedAt",
        2,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StopCodeInterpreterSessionResponse",
            NS,
            "StopCodeInterpreterSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_LAST_UPDATED_AT,
        ],
    );
}

pub static STOP_CODE_INTERPRETER_SESSION_OUTPUT_SCHEMA: &Schema =
    &StopCodeInterpreterSessionOutput::SCHEMA;

impl SerializableStruct for StopCodeInterpreterSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            self.code_interpreter_identifier.as_str(),
        )?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        s.write_timestamp(&Self::M_LAST_UPDATED_AT, &self.last_updated_at)?;
        Ok(())
    }
}

impl GetCodeInterpreterSessionOutput {
    pub const M_CODE_INTERPRETER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$codeInterpreterIdentifier",
            NS,
            "codeInterpreterIdentifier",
        ),
        ShapeType::String,
        "codeInterpreterIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    pub const M_NAME: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$name",
            NS,
            "name",
        ),
        ShapeType::String,
        "name",
        2,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        3,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const M_SESSION_TIMEOUT_SECONDS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$sessionTimeoutSeconds",
            NS,
            "sessionTimeoutSeconds",
        ),
        ShapeType::Integer,
        "sessionTimeoutSeconds",
        4,
    );

    pub const M_STATUS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$status",
            NS,
            "status",
        ),
        ShapeType::String,
        "status",
        5,
    );

    pub const M_CERTIFICATES: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$certificates",
            NS,
            "certificates",
        ),
        ShapeType::List,
        "certificates",
        6,
    );

    pub const M_FILESYSTEM_CONFIGURATIONS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$filesystemConfigurations",
            NS,
            "filesystemConfigurations",
        ),
        ShapeType::List,
        "filesystemConfigurations",
        7,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse",
            NS,
            "GetCodeInterpreterSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_NAME,
            &Self::M_CREATED_AT,
            &Self::M_SESSION_TIMEOUT_SECONDS,
            &Self::M_STATUS,
            &Self::M_CERTIFICATES,
            &Self::M_FILESYSTEM_CONFIGURATIONS,
        ],
    );
}

pub static GET_CODE_INTERPRETER_SESSION_OUTPUT_SCHEMA: &Schema =
    &GetCodeInterpreterSessionOutput::SCHEMA;

impl SerializableStruct for GetCodeInterpreterSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            self.code_interpreter_identifier.as_str(),
        )?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        if let Some(name) = self.name.as_ref() {
            s.write_string(&Self::M_NAME, name.as_str())?;
        }
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        if let Some(timeout) = self.session_timeout_seconds {
            s.write_integer(&Self::M_SESSION_TIMEOUT_SECONDS, timeout)?;
        }
        if let Some(status) = self.status.as_ref() {
            s.write_string(&Self::M_STATUS, status.as_str())?;
        }
        if let Some(certificates) = self.certificates.as_ref() {
            s.write_list(&Self::M_CERTIFICATES, &|list| {
                for certificate in certificates {
                    list.write_struct(&super::Certificate::SCHEMA, certificate)?;
                }
                Ok(())
            })?;
        }
        if let Some(configurations) = self.filesystem_configurations.as_ref() {
            s.write_list(&Self::M_FILESYSTEM_CONFIGURATIONS, &|list| {
                for configuration in configurations {
                    list.write_struct(
                        &super::ToolsFileSystemConfiguration::SCHEMA,
                        configuration,
                    )?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

impl StartBrowserSessionOutput {
    pub const M_BROWSER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse$browserIdentifier",
            NS,
            "browserIdentifier",
        ),
        ShapeType::String,
        "browserIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        2,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const M_STREAMS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse$streams",
            NS,
            "streams",
        ),
        ShapeType::Structure,
        "streams",
        3,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse",
            NS,
            "StartBrowserSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_BROWSER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_CREATED_AT,
            &Self::M_STREAMS,
        ],
    );
}

pub static START_BROWSER_SESSION_OUTPUT_SCHEMA: &Schema = &StartBrowserSessionOutput::SCHEMA;

impl SerializableStruct for StartBrowserSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&Self::M_BROWSER_IDENTIFIER, self.browser_identifier.as_str())?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        if let Some(streams) = self.streams.as_ref() {
            s.write_struct(&Self::M_STREAMS, streams)?;
        }
        Ok(())
    }
}

impl GetBrowserSessionOutput {
    pub const M_BROWSER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$browserIdentifier",
            NS,
            "browserIdentifier",
        ),
        ShapeType::String,
        "browserIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    pub const M_NAME: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$name",
            NS,
            "name",
        ),
        ShapeType::String,
        "name",
        2,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        3,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const M_VIEW_PORT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$viewPort",
            NS,
            "viewPort",
        ),
        ShapeType::Structure,
        "viewPort",
        4,
    );

    pub const M_EXTENSIONS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$extensions",
            NS,
            "extensions",
        ),
        ShapeType::List,
        "extensions",
        5,
    );

    pub const M_ENTERPRISE_POLICIES: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$enterprisePolicies",
            NS,
            "enterprisePolicies",
        ),
        ShapeType::List,
        "enterprisePolicies",
        6,
    );

    pub const M_PROFILE_CONFIGURATION: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$profileConfiguration",
            NS,
            "profileConfiguration",
        ),
        ShapeType::Structure,
        "profileConfiguration",
        7,
    );

    pub const M_SESSION_TIMEOUT_SECONDS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$sessionTimeoutSeconds",
            NS,
            "sessionTimeoutSeconds",
        ),
        ShapeType::Integer,
        "sessionTimeoutSeconds",
        8,
    );

    pub const M_STATUS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$status",
            NS,
            "status",
        ),
        ShapeType::String,
        "status",
        9,
    );

    pub const M_STREAMS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$streams",
            NS,
            "streams",
        ),
        ShapeType::Structure,
        "streams",
        10,
    );

    pub const M_PROXY_CONFIGURATION: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$proxyConfiguration",
            NS,
            "proxyConfiguration",
        ),
        ShapeType::Structure,
        "proxyConfiguration",
        11,
    );

    pub const M_CERTIFICATES: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$certificates",
            NS,
            "certificates",
        ),
        ShapeType::List,
        "certificates",
        12,
    );

    pub const M_FILESYSTEM_CONFIGURATIONS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$filesystemConfigurations",
            NS,
            "filesystemConfigurations",
        ),
        ShapeType::List,
        "filesystemConfigurations",
        13,
    );

    pub const M_SESSION_REPLAY_ARTIFACT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$sessionReplayArtifact",
            NS,
            "sessionReplayArtifact",
        ),
        ShapeType::String,
        "sessionReplayArtifact",
        14,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_LAST_UPDATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$lastUpdatedAt",
            NS,
            "lastUpdatedAt",
        ),
        ShapeType::Timestamp,
        "lastUpdatedAt",
        15,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse",
            NS,
            "GetBrowserSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_BROWSER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_NAME,
            &Self::M_CREATED_AT,
            &Self::M_VIEW_PORT,
            &Self::M_EXTENSIONS,
            &Self::M_ENTERPRISE_POLICIES,
            &Self::M_PROFILE_CONFIGURATION,
            &Self::M_SESSION_TIMEOUT_SECONDS,
            &Self::M_STATUS,
            &Self::M_STREAMS,
            &Self::M_PROXY_CONFIGURATION,
            &Self::M_CERTIFICATES,
            &Self::M_FILESYSTEM_CONFIGURATIONS,
            &Self::M_SESSION_REPLAY_ARTIFACT,
            &Self::M_LAST_UPDATED_AT,
        ],
    );
}

pub static GET_BROWSER_SESSION_OUTPUT_SCHEMA: &Schema = &GetBrowserSessionOutput::SCHEMA;

impl SerializableStruct for GetBrowserSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&Self::M_BROWSER_IDENTIFIER, self.browser_identifier.as_str())?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        if let Some(name) = self.name.as_ref() {
            s.write_string(&Self::M_NAME, name.as_str())?;
        }
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        if let Some(view_port) = self.view_port.as_ref() {
            s.write_struct(&Self::M_VIEW_PORT, view_port)?;
        }
        if let Some(extensions) = self.extensions.as_ref() {
            s.write_list(&Self::M_EXTENSIONS, &|list| {
                for extension in extensions {
                    list.write_struct(&super::BrowserExtension::SCHEMA, extension)?;
                }
                Ok(())
            })?;
        }
        if let Some(policies) = self.enterprise_policies.as_ref() {
            s.write_list(&Self::M_ENTERPRISE_POLICIES, &|list| {
                for policy in policies {
                    list.write_struct(&super::BrowserEnterprisePolicy::SCHEMA, policy)?;
                }
                Ok(())
            })?;
        }
        if let Some(profile) = self.profile_configuration.as_ref() {
            s.write_struct(&Self::M_PROFILE_CONFIGURATION, profile)?;
        }
        if let Some(timeout) = self.session_timeout_seconds {
            s.write_integer(&Self::M_SESSION_TIMEOUT_SECONDS, timeout)?;
        }
        if let Some(status) = self.status.as_ref() {
            s.write_string(&Self::M_STATUS, status.as_str())?;
        }
        if let Some(streams) = self.streams.as_ref() {
            s.write_struct(&Self::M_STREAMS, streams)?;
        }
        if let Some(proxy) = self.proxy_configuration.as_ref() {
            s.write_struct(&Self::M_PROXY_CONFIGURATION, proxy)?;
        }
        if let Some(certificates) = self.certificates.as_ref() {
            s.write_list(&Self::M_CERTIFICATES, &|list| {
                for certificate in certificates {
                    list.write_struct(&super::Certificate::SCHEMA, certificate)?;
                }
                Ok(())
            })?;
        }
        if let Some(configurations) = self.filesystem_configurations.as_ref() {
            s.write_list(&Self::M_FILESYSTEM_CONFIGURATIONS, &|list| {
                for configuration in configurations {
                    list.write_struct(
                        &super::ToolsFileSystemConfiguration::SCHEMA,
                        configuration,
                    )?;
                }
                Ok(())
            })?;
        }
        if let Some(artifact) = self.session_replay_artifact.as_ref() {
            s.write_string(&Self::M_SESSION_REPLAY_ARTIFACT, artifact.as_str())?;
        }
        if let Some(last_updated_at) = self.last_updated_at.as_ref() {
            s.write_timestamp(&Self::M_LAST_UPDATED_AT, last_updated_at)?;
        }
        Ok(())
    }
}
