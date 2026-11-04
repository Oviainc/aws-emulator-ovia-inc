// SPDX-License-Identifier: CC0-1.0

//! The schema and the serializer an ExperimentRun document needs, because its date-time
//! members cannot be rendered from a macro-built schema.
//!
//! `ExperimentRun$StartedAt`, `ExperimentRun$UpdatedAt` and `ExperimentRun$EndedAt` all
//! target `com.amazonaws.appconfig#Iso8601DateTime`, a `timestamp` shape carrying
//! `@timestampFormat("date-time")` (`smithy_query(query="shape", shape="ExperimentRun",
//! model_path="services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json")`
//! reports `"timestampFormat": "date-time"` on all three), so each is an RFC 3339 /
//! ISO-8601 STRING on the restJson1 wire this service speaks.
//! `types/deployment_wire.rs` states why a macro-built member schema cannot say so, and
//! holds the macros that can; this module states the `ExperimentRun` member list against
//! them.
//!
//! `StartExperimentRun`, `StopExperimentRun`, `GetExperimentRun` and `UpdateExperimentRun`
//! all answer that one shape and the scaffold declares one Rust struct per operation, so
//! the member list is stated ONCE here rather than once per struct: four documents of the
//! same shape cannot then disagree about a member, an order or a wire form.

/// The `ExperimentRun` shape's schema and serializer, for one of the Rust structs that
/// carries it.
///
/// Member set, order and requiredness from `smithy_query(query="shape",
/// shape="ExperimentRun")`: `Run` carries `@default` and every other member is optional,
/// `ExposurePercentage` targets the `NullablePercentage` float, `TreatmentOverrides` is a
/// UNION, and `Result` and `ExperimentDefinitionSnapshot` are structures.
macro_rules! ac_experiment_run_shape {
    ($ty:ident, $schema_static:ident) => {
        ac_schema_and_codec!(
            $ty,
            $schema_static,
            "com.amazonaws.appconfig#ExperimentRun",
            "com.amazonaws.appconfig",
            "ExperimentRun",
            (M_APPLICATION_ID, application_id, "ApplicationId", 0, opt, string),
            (
                M_EXPERIMENT_DEFINITION_ID,
                experiment_definition_id,
                "ExperimentDefinitionId",
                1,
                opt,
                string
            ),
            (M_RUN, run, "Run", 2, def, integer),
            (M_DESCRIPTION, description, "Description", 3, opt, string),
            (M_STATUS, status, "Status", 4, opt, string),
            (
                M_EXPOSURE_PERCENTAGE,
                exposure_percentage,
                "ExposurePercentage",
                5,
                opt,
                float
            ),
            (
                M_TREATMENT_OVERRIDES,
                treatment_overrides,
                "TreatmentOverrides",
                6,
                opt,
                union_of(TreatmentOverrides)
            ),
            (M_RESULT, result, "Result", 7, opt, struct_of(ExperimentRunResult)),
            (M_STARTED_AT, started_at, "StartedAt", 8, opt, date_time),
            (M_UPDATED_AT, updated_at, "UpdatedAt", 9, opt, date_time),
            (M_ENDED_AT, ended_at, "EndedAt", 10, opt, date_time),
            (
                M_EXPERIMENT_DEFINITION_SNAPSHOT,
                experiment_definition_snapshot,
                "ExperimentDefinitionSnapshot",
                11,
                opt,
                struct_of(ExperimentDefinitionSnapshot)
            ),
        );
    };
}
