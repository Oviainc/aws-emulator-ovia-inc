// SPDX-License-Identifier: CC0-1.0

//! The schema and the serializer an ExperimentDefinition document needs, because its
//! date-time members cannot be rendered from a macro-built schema.
//!
//! `ExperimentDefinition$CreatedAt` and `ExperimentDefinition$UpdatedAt` target
//! `com.amazonaws.appconfig#Iso8601DateTime`, a `timestamp` shape carrying
//! `@timestampFormat("date-time")` (`smithy_query(query="shape",
//! shape="ExperimentDefinition",
//! model_path="services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json")`
//! reports `"timestampFormat": "date-time"` on both), so each is an RFC 3339 / ISO-8601
//! STRING on the restJson1 wire this service speaks. `types/deployment_wire.rs` states why
//! a macro-built member schema cannot say so, and holds the macros that can; this module
//! states the `ExperimentDefinition` member list against them.
//!
//! `CreateExperimentDefinition`, `GetExperimentDefinition` and
//! `UpdateExperimentDefinition` all answer that one shape and the scaffold declares one
//! Rust struct per operation, so the member list is stated ONCE here rather than once per
//! struct: three documents of the same shape cannot then disagree about a member, an order
//! or a wire form.

/// The `ExperimentDefinition` shape's schema and serializer, for one of the Rust structs
/// that carries it.
///
/// Member set, order and requiredness from `smithy_query(query="shape",
/// shape="ExperimentDefinition")`: every member is optional on this output shape, `Control`
/// targets the `Treatment` structure and `Treatments` the `TreatmentList` of it.
macro_rules! ac_experiment_definition_shape {
    ($ty:ident, $schema_static:ident) => {
        ac_schema_and_codec!(
            $ty,
            $schema_static,
            "com.amazonaws.appconfig#ExperimentDefinition",
            "com.amazonaws.appconfig",
            "ExperimentDefinition",
            (M_APPLICATION_ID, application_id, "ApplicationId", 0, opt, string),
            (M_ID, id, "Id", 1, opt, string),
            (M_NAME, name, "Name", 2, opt, string),
            (M_HYPOTHESIS, hypothesis, "Hypothesis", 3, opt, string),
            (M_STATUS, status, "Status", 4, opt, string),
            (
                M_CONFIGURATION_PROFILE_ID,
                configuration_profile_id,
                "ConfigurationProfileId",
                5,
                opt,
                string
            ),
            (M_ENVIRONMENT_ID, environment_id, "EnvironmentId", 6, opt, string),
            (M_FLAG_KEY, flag_key, "FlagKey", 7, opt, string),
            (M_AUDIENCE_RULE, audience_rule, "AudienceRule", 8, opt, string),
            (
                M_AUDIENCE_DESCRIPTION,
                audience_description,
                "AudienceDescription",
                9,
                opt,
                string
            ),
            (M_LAUNCH_CRITERIA, launch_criteria, "LaunchCriteria", 10, opt, string),
            (M_TREATMENTS, treatments, "Treatments", 11, opt, list_of(Treatment)),
            (M_CONTROL, control, "Control", 12, opt, struct_of(Treatment)),
            (M_CREATED_AT, created_at, "CreatedAt", 13, opt, date_time),
            (M_UPDATED_AT, updated_at, "UpdatedAt", 14, opt, date_time),
            (
                M_KMS_KEY_IDENTIFIER,
                kms_key_identifier,
                "KmsKeyIdentifier",
                15,
                opt,
                string
            ),
        );
    };
}
