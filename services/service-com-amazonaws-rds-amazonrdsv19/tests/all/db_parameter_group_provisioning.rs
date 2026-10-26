// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PREREQUISITE role a DB parameter group plays: a group
//! `CreateDBParameterGroup` persisted is what a DB instance created with non-default
//! parameters associates with, and the association every later read reports.
//!
//! The sibling suites pin each operation of this slice on its own — the group's own
//! create-then-read envelope (`db_parameter_groups`), the parameters a group carries
//! (`db_parameters`), the default group an instance that names none gets
//! (`db_instance_envelope`) and the refusal to delete a group in use
//! (`db_parameter_group_deletion`). What is pinned HERE is the CHAIN across them: a
//! created group must be a real, family-scoped record that a later `CreateDBInstance`
//! can be provisioned against, rather than an answer the service declines to serve.
//!
//! The rule is asserted over a TABLE spanning the class it generalises over — the
//! engine, the engine version the create resolves (its engine's default, or a major
//! version the request names), the shape of the parameter-group family that version
//! publishes, and the case the group's name is created and referenced in. Nothing in
//! the table hard-codes a family: each row reads the family out of the DEFAULT group
//! the service itself reports for an instance of that engine and version, so the
//! group a row creates is always the family the instance's own engine version
//! publishes.

use service_com_amazonaws_rds_amazonrdsv19::state::default_db_parameter_group_name;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_ok, subtrees, text, texts, wait_for_settle, REGION,
};
// The one ARN shape this crate's suites assert for a DB parameter group, owned by the
// suite that pins the group's own envelope.
use crate::db_parameter_groups::expected_arn;

/// The `ParameterApplyStatus` RDS reports for a parameter group that is in effect.
const IN_SYNC: &str = "in-sync";

/// One row of the prerequisite table.
struct PrereqCase {
    case: &'static str,
    /// The `Engine` both instances of the row are created with.
    engine: &'static str,
    /// `EngineVersion` as the request names it, or `None` when it names none and the
    /// engine's own default version is resolved.
    requested_version: Option<&'static str>,
    /// The `DBParameterGroupName` the create sends — its case is part of the class.
    requested_group: &'static str,
    /// The spelling the DB instance refers to the same group by.
    named_by_instance: &'static str,
    /// Distinguishes this row's two DB instance identifiers from every other row's.
    slug: &'static str,
}

/// The class the rule generalises over: one row per database product this front door
/// serves (so one per shape of `DBParameterGroupFamily` — `mysqlX.Y`, `mariadbX.Y`,
/// `postgresNN`, `oracle-ee-NN`, `sqlserver-ex-NN.0`), both branches of
/// `CreateDBInstanceMessage$EngineVersion` ("If you don't specify a value, then Amazon
/// RDS uses the default DB parameter group for the specified DB engine and version"),
/// and a group name sent lowercase, mixed-case and upper-case.
const PREREQ_CASES: &[PrereqCase] = &[
    PrereqCase {
        case: "mysql at its engine default, the group named as created",
        engine: "mysql",
        requested_version: None,
        requested_group: "emu-prereq-mysql",
        named_by_instance: "emu-prereq-mysql",
        slug: "mysql-default",
    },
    PrereqCase {
        case: "mariadb at its engine default, the group named in another case",
        engine: "mariadb",
        requested_version: None,
        requested_group: "Emu-Prereq-MariaDB",
        named_by_instance: "emu-prereq-mariadb",
        slug: "mariadb-default",
    },
    PrereqCase {
        case: "postgres at its engine default, the group created upper-case",
        engine: "postgres",
        requested_version: None,
        requested_group: "EMU-PREREQ-POSTGRES",
        named_by_instance: "EMU-PREREQ-POSTGRES",
        slug: "postgres-default",
    },
    PrereqCase {
        case: "postgres at a major version the request names",
        engine: "postgres",
        requested_version: Some("16"),
        requested_group: "emu-prereq-postgres-16",
        named_by_instance: "Emu-Prereq-Postgres-16",
        slug: "postgres-16",
    },
    PrereqCase {
        case: "oracle-ee at its engine default",
        engine: "oracle-ee",
        requested_version: None,
        requested_group: "emu-prereq-oracle",
        named_by_instance: "emu-prereq-oracle",
        slug: "oracle-default",
    },
    PrereqCase {
        case: "sqlserver-ex, whose family carries a two-part version",
        engine: "sqlserver-ex",
        requested_version: None,
        requested_group: "Emu-Prereq-SqlServer",
        named_by_instance: "EMU-PREREQ-SQLSERVER",
        slug: "sqlserver-default",
    },
];

/// The description a row's group is created with — one per row, so the read-back cannot
/// pass on another row's record.
fn description_of(row: &PrereqCase) -> String {
    format!("parameters for {}", row.slug)
}

/// `CreateDBInstance` for a row, optionally naming a DB parameter group.
async fn create_instance(
    router: &MultiServiceRouter,
    row: &PrereqCase,
    identifier: &str,
    group: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", row.engine.to_string()),
        ("DBInstanceClass", "db.m5.large".to_string()),
    ];
    if let Some(version) = row.requested_version {
        params.push(("EngineVersion", version.to_string()));
    }
    if let Some(group) = group {
        params.push(("DBParameterGroupName", group.to_string()));
    }
    call(router, "CreateDBInstance", &params).await
}

/// `DescribeDBInstances` naming one instance, as the one `<DBInstance>` subtree it
/// reports.
async fn describe_instance(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    );
    let found = subtrees(&body, "DBInstance");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBInstance for {identifier}, got {body}"
    );
    found.into_iter().next().expect("checked just above")
}

/// The one parameter-group association an instance envelope reports, as
/// `(DBParameterGroupName, ParameterApplyStatus)`.
fn sole_association(case: &str, envelope: &str, what: &str) -> (String, String) {
    // `DBInstance$DBParameterGroups` is a list of `DBParameterGroupStatus`, which the
    // awsQuery protocol writes under the list member's own name, `DBParameterGroup`.
    let found = subtrees(envelope, "DBParameterGroup");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: the {what} envelope must report exactly one parameter-group \
         membership, got {envelope}"
    );
    let membership = found.into_iter().next().expect("checked just above");
    (
        text(&membership, "DBParameterGroupName"),
        text(&membership, "ParameterApplyStatus"),
    )
}

/// What one row derived while it was being provisioned, so the reads after the
/// transition settles can be asserted against it.
struct Provisioned {
    /// The DB parameter group family the row's engine version publishes, read off the
    /// DEFAULT group the service reported for an instance of it.
    family: String,
    /// The default group that same instance was placed in.
    default_group: String,
    /// The instance created WITHOUT a group — the control for the row.
    default_instance: String,
    /// The instance created naming the row's own group.
    custom_instance: String,
    /// The group's name as the store keys it.
    stored_group: String,
}

#[tokio::test]
async fn a_persisted_group_is_the_prerequisite_a_non_default_db_instance_is_built_on() {
    let router = emulator();
    let mut provisioned = Vec::new();

    for row in PREREQ_CASES {
        let default_instance = format!("emu-prereq-{}-base", row.slug);
        let custom_instance = format!("emu-prereq-{}-custom", row.slug);
        let stored_group = row.requested_group.to_ascii_lowercase();
        let description = description_of(row);

        // The control instance names no group, so the service places it in the default
        // group of the version it resolved. That name is where this row's family comes
        // from: the family is read out of the service's own catalogue rather than
        // written into the table.
        let created = expect_ok(
            row.case,
            create_instance(&router, row, &default_instance, None).await,
        );
        let (default_group, apply_status) = sole_association(row.case, &created, "create");
        assert_eq!(
            apply_status, IN_SYNC,
            "case {:?}: the default group's membership must be reported {IN_SYNC}, got \
             {apply_status:?}",
            row.case
        );
        let prefix = default_db_parameter_group_name("");
        let family = default_group
            .strip_prefix(&prefix)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the default group of an engine version must be {prefix:?} \
                     followed by its family, got {default_group:?}",
                    row.case
                )
            })
            .to_string();
        assert!(
            !family.is_empty(),
            "case {:?}: the default group {default_group:?} must name a family",
            row.case
        );

        // The group the issue's provisioning chain starts from: created for exactly the
        // family the instance's own engine version publishes, so an instance can be
        // built on it.
        let created_group = expect_ok(
            row.case,
            call(
                &router,
                "CreateDBParameterGroup",
                &[
                    ("DBParameterGroupName", row.requested_group),
                    ("DBParameterGroupFamily", family.as_str()),
                    ("Description", description.as_str()),
                ],
            )
            .await,
        );
        for (member, expected) in [
            ("DBParameterGroupName", stored_group.as_str()),
            ("DBParameterGroupFamily", family.as_str()),
            ("Description", description.as_str()),
            ("DBParameterGroupArn", &expected_arn(REGION, &stored_group)),
        ] {
            let actual = text(&created_group, member);
            assert_eq!(
                actual, expected,
                "case {:?}: the create must report {member} as {expected:?}, got \
                 {actual:?}",
                row.case
            );
        }
        // A created group is not one of the groups the service mints per engine
        // version, so the instance built on it really is running non-default
        // parameters.
        assert_ne!(
            stored_group, default_group,
            "case {:?}: a created group must differ from the engine version's default \
             group",
            row.case
        );

        // The instance the chain exists for: created naming the group, in whatever case
        // this row spells it.
        let created = expect_ok(
            row.case,
            create_instance(
                &router,
                row,
                &custom_instance,
                Some(row.named_by_instance),
            )
            .await,
        );
        let (associated, apply_status) = sole_association(row.case, &created, "create");
        assert!(
            associated.eq_ignore_ascii_case(&stored_group),
            "case {:?}: the create must report the instance in the group it named, \
             expected {stored_group:?} however it is cased, got {associated:?}",
            row.case
        );
        assert_eq!(
            apply_status, IN_SYNC,
            "case {:?}: the created group's membership must be reported {IN_SYNC}, got \
             {apply_status:?}",
            row.case
        );

        provisioned.push(Provisioned {
            family,
            default_group,
            default_instance,
            custom_instance,
            stored_group,
        });
    }

    // Both instances of every row are still in `creating` until the span this service
    // owes has passed; waiting it out once is what makes the reads below the reads a
    // caller makes of an `available` instance.
    wait_for_settle();

    for (row, state) in PREREQ_CASES.iter().zip(&provisioned) {
        // The association is read back from the record, not from the create's answer:
        // the instance built on the created group reports that group, and the control
        // instance beside it still reports the default one.
        let envelope = describe_instance(&router, row.case, &state.custom_instance).await;
        let (associated, apply_status) = sole_association(row.case, &envelope, "describe");
        assert!(
            associated.eq_ignore_ascii_case(&state.stored_group),
            "case {:?}: the describe must report the instance in the group it was \
             created with, expected {:?} however it is cased, got {associated:?}",
            row.case,
            state.stored_group
        );
        assert_eq!(
            apply_status, IN_SYNC,
            "case {:?}: the describe must report the membership {IN_SYNC}, got \
             {apply_status:?}",
            row.case
        );

        let control = describe_instance(&router, row.case, &state.default_instance).await;
        let (default_group, _) = sole_association(row.case, &control, "control describe");
        assert_eq!(
            default_group, state.default_group,
            "case {:?}: creating a group must leave the instance that named none in \
             the default group",
            row.case
        );

        // And the group itself is still the family-scoped record the create persisted,
        // read back through the group report the same way the chain's next step would.
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBParameterGroups",
                &[("DBParameterGroupName", state.stored_group.as_str())],
            )
            .await,
        );
        for (member, expected) in [
            ("DBParameterGroupName", state.stored_group.as_str()),
            ("DBParameterGroupFamily", state.family.as_str()),
            ("Description", &description_of(row)),
            (
                "DBParameterGroupArn",
                &expected_arn(REGION, &state.stored_group),
            ),
        ] {
            let actual = text(&described, member);
            assert_eq!(
                actual, expected,
                "case {:?}: the group report must report {member} as {expected:?}, got \
                 {actual:?}",
                row.case
            );
        }
        // The chain's next step reaches the same record through the PARAMETER report,
        // where a group nothing has modified yet carries no parameter of its own: the
        // defaults belong to the engine family, which publishes them.
        let own = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBParameters",
                &[
                    ("DBParameterGroupName", state.stored_group.as_str()),
                    ("Source", "user"),
                ],
            )
            .await,
        );
        let parameters = texts(&own, "ParameterName");
        assert!(
            parameters.is_empty(),
            "case {:?}: a group nothing modified must carry no parameter of its own, \
             got {parameters:?}",
            row.case
        );
    }
}
