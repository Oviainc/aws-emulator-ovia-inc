// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the parameters Amazon RDS ADDS to the PostgreSQL engine — the ones
//! in its own `rds.` namespace — as a DB (instance) parameter group family publishes them.
//!
//! RULE: a DB parameter group family reports every parameter its engine publishes, the
//! RDS-specific ones included, each with the metadata the engine documents (`ApplyType`,
//! `DataType`, `AllowedValues`, `IsModifiable`) and `Source` `engine-default`; where the
//! engine's documented DEFAULT moved between versions, the value reported for a family is
//! the one that family's own version carries.
//!
//! RDS for PostgreSQL's `rds.force_ssl` is the parameter both halves of that rule are about:
//! it is boolean and admits `0,1`, and BOTH its apply type and its default moved at
//! PostgreSQL 15 — from that release the engine applies a change to it without a restart
//! (`dynamic`, so a change takes the immediate apply method) and RDS publishes its default as
//! 1, where the versions before it publish `static` and 0. The default is also the fact that
//! decides whether a session on an instance of that family may be in the clear.
//!
//! The expected records are the engine's own published ones (the Amazon RDS User Guide's
//! "Parameters for RDS for PostgreSQL" table, which publishes each parameter's apply type,
//! data type, allowed values and default), written here as LITERALS so a drift in this
//! service's catalogue fails these rows rather than moving with them. The rows span the
//! published version bound (15) and versions either side of it, families no issue cited,
//! another PostgreSQL-compatible engine (which runs the same releases, so it takes the apply
//! type of its version and keeps its own published default of 0), both readers of the one
//! catalogue, and a second `rds.` parameter whose record does not move.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    APPLY_METHOD_IMMEDIATE, APPLY_METHOD_PENDING_REBOOT, APPLY_TYPE_DYNAMIC, APPLY_TYPE_STATIC,
    PARAMETER_SOURCE_ENGINE_DEFAULT, PARAMETER_SOURCE_USER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, text};
use crate::engine_default_parameters::{reported, Reported};

/// The error code Amazon RDS answers a parameter value outside a parameter's published
/// domain with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The `rds.` parameter the TLS requirement of an RDS for PostgreSQL instance is carried by.
const FORCE_SSL: &str = "rds.force_ssl";

/// The one group AWS itself provides for `family`, which no request creates.
fn default_group(family: &str) -> String {
    format!("default.{family}")
}

/// The one reported `<Parameter>` named `name`, from a report filtered down to it.
fn only_entry(case: &str, body: &str, name: &str) -> Reported {
    let entries = reported(body);
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected exactly the parameter {name}, got {:?}",
        entries.iter().map(|entry| &entry.name).collect::<Vec<_>>()
    );
    let entry = entries.into_iter().next().expect("checked just above");
    assert_eq!(
        entry.name, name,
        "case {case:?}: expected the parameter {name}, got {:?}",
        entry.name
    );
    entry
}

/// `DescribeDBParameters` on `group`, filtered to the parameter `name`.
async fn group_parameter(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    name: &str,
) -> Reported {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBParameters",
            &[
                ("DBParameterGroupName", group.to_string()),
                ("Filters.Filter.1.Name", "parameter-name".to_string()),
                ("Filters.Filter.1.Values.Value.1", name.to_string()),
            ],
        )
        .await,
    );
    only_entry(case, &body, name)
}

/// `DescribeEngineDefaultParameters` for `family`, filtered to the parameter `name`.
async fn family_parameter(
    router: &MultiServiceRouter,
    case: &str,
    family: &str,
    name: &str,
) -> Reported {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeEngineDefaultParameters",
            &[
                ("DBParameterGroupFamily", family.to_string()),
                ("Filters.Filter.1.Name", "parameter-name".to_string()),
                ("Filters.Filter.1.Values.Value.1", name.to_string()),
            ],
        )
        .await,
    );
    only_entry(case, &body, name)
}

/// The published record one row expects: `(ApplyType, DataType, AllowedValues,
/// ParameterValue, IsModifiable, Source)`.
type Declared<'row> = (
    Option<&'row str>,
    Option<&'row str>,
    Option<&'row str>,
    Option<&'row str>,
    Option<&'row str>,
    Option<&'row str>,
);

/// The record `entry` reports, read the same way for either reader.
fn declared(entry: &Reported) -> Declared<'_> {
    (
        entry.apply_type.as_deref(),
        entry.data_type.as_deref(),
        entry.allowed_values.as_deref(),
        entry.value.as_deref(),
        entry.is_modifiable.as_deref(),
        entry.source.as_deref(),
    )
}

/// One row of the published table: the family, the parameter, and the record the engine
/// publishes for it in that family.
struct Row {
    case: &'static str,
    family: &'static str,
    parameter: &'static str,
    apply_type: &'static str,
    data_type: &'static str,
    allowed_values: &'static str,
    default_value: &'static str,
}

/// The table: `rds.force_ssl` over the published version bound and a version either side of
/// it, `rds.logical_replication` whose default does not move, and another
/// PostgreSQL-compatible engine, whose own published default is the catalogue's.
const ROWS: &[Row] = &[
    Row {
        case: "the family the recorded case read",
        family: "postgres17",
        parameter: FORCE_SSL,
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "boolean",
        allowed_values: "0,1",
        default_value: "1",
    },
    Row {
        case: "another family past the bound, which no issue cited",
        family: "postgres16",
        parameter: FORCE_SSL,
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "boolean",
        allowed_values: "0,1",
        default_value: "1",
    },
    Row {
        case: "the published bound itself: the first version whose default is on",
        family: "postgres15",
        parameter: FORCE_SSL,
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "boolean",
        allowed_values: "0,1",
        default_value: "1",
    },
    Row {
        case: "one version before the bound, where it is static and off",
        family: "postgres14",
        parameter: FORCE_SSL,
        apply_type: APPLY_TYPE_STATIC,
        data_type: "boolean",
        allowed_values: "0,1",
        default_value: "0",
    },
    Row {
        case: "an older family still, where it is static and off",
        family: "postgres13",
        parameter: FORCE_SSL,
        apply_type: APPLY_TYPE_STATIC,
        data_type: "boolean",
        allowed_values: "0,1",
        default_value: "0",
    },
    Row {
        case: "another PostgreSQL-compatible engine past the bound, with its own default",
        family: "aurora-postgresql16",
        parameter: FORCE_SSL,
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "boolean",
        allowed_values: "0,1",
        default_value: "0",
    },
    Row {
        case: "that engine before the bound, where it is static",
        family: "aurora-postgresql13",
        parameter: FORCE_SSL,
        apply_type: APPLY_TYPE_STATIC,
        data_type: "boolean",
        allowed_values: "0,1",
        default_value: "0",
    },
    Row {
        case: "a second RDS-namespace parameter, whose default does not move",
        family: "postgres17",
        parameter: "rds.logical_replication",
        apply_type: APPLY_TYPE_STATIC,
        data_type: "boolean",
        allowed_values: "0,1",
        default_value: "0",
    },
    Row {
        case: "the same parameter in an older family",
        family: "postgres13",
        parameter: "rds.logical_replication",
        apply_type: APPLY_TYPE_STATIC,
        data_type: "boolean",
        allowed_values: "0,1",
        default_value: "0",
    },
];

/// RULE: both readers of a family's catalogue report each RDS-namespace parameter with the
/// record the engine publishes for it in that family — the service-provided default group of
/// the family, and the family-level report.
#[tokio::test]
async fn a_postgres_family_publishes_its_rds_namespace_parameters() {
    let router = emulator();

    for row in ROWS {
        let expected = (
            Some(row.apply_type),
            Some(row.data_type),
            Some(row.allowed_values),
            Some(row.default_value),
            Some("true"),
            Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
        );
        let group = default_group(row.family);
        for (label, entry) in [
            (
                "DescribeDBParameters",
                group_parameter(&router, row.case, &group, row.parameter).await,
            ),
            (
                "DescribeEngineDefaultParameters",
                family_parameter(&router, row.case, row.family, row.parameter).await,
            ),
        ] {
            assert_eq!(
                declared(&entry),
                expected,
                "case {:?} ({label}): expected the record {} publishes for {}, got {entry:?}",
                row.case,
                row.family,
                row.parameter
            );
            assert!(
                entry
                    .description
                    .as_ref()
                    .is_some_and(|text| !text.is_empty()),
                "case {:?} ({label}): a published parameter reports its Description, got {:?}",
                row.case,
                entry.description
            );
        }
    }
}

/// RULE, the other half: the published record is the domain a WRITE is held to, and a value
/// a caller sets is reported as the caller's own over the family's default — under the apply
/// METHOD that family's published apply type implies for a change nobody asked a method for
/// ("changes to dynamic parameters are applied immediately", "changes to static parameters
/// require a reboot").
///
/// The rows differ along the family — one past the version bound, where the parameter is
/// dynamic and defaults on, and one before it, where it is static and defaults off — and
/// along the value set, so neither row can pass by reporting the default.
#[tokio::test]
async fn a_caller_set_force_ssl_is_reported_over_the_families_default() {
    let router = emulator();

    // (case, family, the family's published default, the value set, the apply type that
    // family publishes, the apply method a change then takes)
    let rows: [(&str, &str, &str, &str, &str, &str); 2] = [
        (
            "a family where it is dynamic and on, set off",
            "postgres17",
            "1",
            "0",
            APPLY_TYPE_DYNAMIC,
            APPLY_METHOD_IMMEDIATE,
        ),
        (
            "a family where it is static and off, set on",
            "postgres14",
            "0",
            "1",
            APPLY_TYPE_STATIC,
            APPLY_METHOD_PENDING_REBOOT,
        ),
    ];

    for (row, (case, family, default_value, set, apply_type, apply_method)) in
        rows.into_iter().enumerate()
    {
        let group = format!("emu-rdsns-{row}-{}", family.replace('.', "-"));
        expect_ok(
            case,
            call(
                &router,
                "CreateDBParameterGroup",
                &[
                    ("DBParameterGroupName", group.clone()),
                    ("DBParameterGroupFamily", family.to_string()),
                    ("Description", "the rds namespace under test".to_string()),
                ],
            )
            .await,
        );

        // Before the write, the group reports the family's own default.
        let entry = group_parameter(&router, case, &group, FORCE_SSL).await;
        assert_eq!(
            (entry.value.as_deref(), entry.source.as_deref()),
            (Some(default_value), Some(PARAMETER_SOURCE_ENGINE_DEFAULT)),
            "case {case:?}: a fresh group reports the family's default, got {entry:?}"
        );

        expect_ok(
            case,
            call(
                &router,
                "ModifyDBParameterGroup",
                &[
                    ("DBParameterGroupName", group.clone()),
                    ("Parameters.Parameter.1.ParameterName", FORCE_SSL.to_string()),
                    ("Parameters.Parameter.1.ParameterValue", set.to_string()),
                ],
            )
            .await,
        );

        // After it, the value is the caller's, and it carries the apply method its family's
        // published apply type implies when the caller asks for none.
        let entry = group_parameter(&router, case, &group, FORCE_SSL).await;
        assert_eq!(
            (
                entry.value.as_deref(),
                entry.source.as_deref(),
                entry.apply_type.as_deref(),
                entry.apply_method.as_deref(),
            ),
            (
                Some(set),
                Some(PARAMETER_SOURCE_USER),
                Some(apply_type),
                Some(apply_method),
            ),
            "case {case:?}: expected the value the caller set, got {entry:?}"
        );

        // A value outside the published domain is refused, naming that domain.
        let answered = expect_error(
            case,
            call(
                &router,
                "ModifyDBParameterGroup",
                &[
                    ("DBParameterGroupName", group.clone()),
                    ("Parameters.Parameter.1.ParameterName", FORCE_SSL.to_string()),
                    ("Parameters.Parameter.1.ParameterValue", "on".to_string()),
                ],
            )
            .await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        let message = text(&answered, "Message");
        let expected =
            format!("Invalid parameter value: on for: {FORCE_SSL} allowed values are: 0,1");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}
