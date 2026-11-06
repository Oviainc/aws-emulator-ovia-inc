// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteIntegration`: the operation that deletes a zero-ETL
//! integration this service holds.
//!
//! The rules under test:
//!
//! * a delete acts on the integration the identifier NAMES — the name a create gave it or
//!   its ARN — and answers with that integration in the `deleting` status, after which the
//!   identifier reports no integration at all;
//! * an identifier that names no integration the account holds is the modelled 404,
//!   whichever form it took, so a caller can tell "deleted" from "never existed";
//! * an integration this service is still bringing up is not one a delete can act on: that
//!   is the modelled state fault.
//!
//! The rules are asserted over TABLES spanning what they generalise over: the source of the
//! integration (a DB cluster and a DB instance), the two forms its identifier takes, and
//! one row per way an identifier can name nothing.
//!
//! Every call goes over the wire: real awsQuery form POSTs through a `MultiServiceRouter`
//! carrying the SigV4 credential scope the router keys on, and every assertion reads the
//! XML the emulator answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, text, texts, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// An Amazon Redshift namespace ARN, the kind of target `CreateIntegration$TargetArn` takes.
const REDSHIFT_TARGET_ARN: &str =
    "arn:aws:redshift-serverless:us-west-2:000000000000:namespace/zero-etl";

/// One row of the delete table: the source database the integration replicates, and how the
/// delete names the integration.
struct DeleteCase {
    case: &'static str,
    name: &'static str,
    /// Whether the source is a DB cluster (otherwise a DB instance).
    source_is_a_cluster: bool,
    /// Whether the delete names the integration by its ARN rather than by its name.
    by_arn: bool,
}

/// The class the delete rule generalises over: both kinds of source a zero-ETL integration
/// can replicate, and both forms "the unique identifier of the integration" takes.
const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        case: "a cluster-sourced integration deleted by name",
        name: "del-integration-cluster",
        source_is_a_cluster: true,
        by_arn: false,
    },
    DeleteCase {
        case: "an instance-sourced integration deleted by ARN",
        name: "del-integration-instance",
        source_is_a_cluster: false,
        by_arn: true,
    },
];

/// Create the source database a row replicates and return its ARN, waiting out the span its
/// create owes so it is `available`.
async fn source_database(router: &MultiServiceRouter, row: &DeleteCase) -> String {
    let identifier = format!("{}-src", row.name);
    if row.source_is_a_cluster {
        expect_ok(
            row.case,
            call(
                router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier.as_str()),
                    ("Engine", "aurora-mysql"),
                ],
            )
            .await,
        );
        wait_for_settle();
        return format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}");
    }
    expect_ok(
        row.case,
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier.as_str()),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.t3.micro"),
            ],
        )
        .await,
    );
    wait_for_settle();
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:{identifier}")
}

/// `DescribeIntegrations` naming one integration.
async fn describe(router: &MultiServiceRouter, identifier: &str) -> (u16, String) {
    call(
        router,
        "DescribeIntegrations",
        &[("IntegrationIdentifier", identifier)],
    )
    .await
}

#[tokio::test]
async fn a_delete_takes_the_integration_the_identifier_names_out_of_the_report() {
    for row in DELETE_CASES {
        // Each row gets its own emulator, so one row's transitions cannot carry into the
        // next.
        let router = emulator();
        let case = row.case;
        let source_arn = source_database(&router, row).await;

        let created = expect_ok(
            case,
            call(
                &router,
                "CreateIntegration",
                &[
                    ("IntegrationName", row.name),
                    ("SourceArn", source_arn.as_str()),
                    ("TargetArn", REDSHIFT_TARGET_ARN),
                    ("Description", "the integration under test"),
                ],
            )
            .await,
        );
        let arn = text(&created, "IntegrationArn");
        // An integration that is still being brought up is not one a delete can act on.
        expect_error(
            case,
            call(
                &router,
                "DeleteIntegration",
                &[("IntegrationIdentifier", row.name)],
            )
            .await,
            400,
            "InvalidIntegrationStateFault",
        );
        wait_for_settle();

        let identifier = if row.by_arn { arn.as_str() } else { row.name };
        let deleted = expect_ok(
            case,
            call(
                &router,
                "DeleteIntegration",
                &[("IntegrationIdentifier", identifier)],
            )
            .await,
        );
        // The answer is the integration being deleted, projected the way the report
        // projects it, so the delete and the report cannot disagree about one record.
        assert_eq!(
            (
                text(&deleted, "IntegrationName"),
                text(&deleted, "IntegrationArn"),
                text(&deleted, "SourceArn"),
                text(&deleted, "TargetArn"),
                text(&deleted, "Status"),
                text(&deleted, "Description"),
            ),
            (
                row.name.to_string(),
                arn.clone(),
                source_arn.clone(),
                REDSHIFT_TARGET_ARN.to_string(),
                "deleting".to_string(),
                "the integration under test".to_string(),
            ),
            "case {case:?}: got {deleted}"
        );
        // Within the span the delete owes, the integration is still reported — in the
        // status the delete put it in.
        let reported = expect_ok(case, describe(&router, row.name).await);
        assert_eq!(
            text(&reported, "Status"),
            "deleting",
            "case {case:?}: got {reported}"
        );

        // Once the clock has made the removal due, the identifier names nothing: by name
        // and by ARN alike.
        wait_for_settle();
        for (form, named) in [("by name", row.name), ("by ARN", arn.as_str())] {
            expect_error(
                &format!("{case}: {form}"),
                describe(&router, named).await,
                404,
                "IntegrationNotFoundFault",
            );
            expect_error(
                &format!("{case}: {form}"),
                call(
                    &router,
                    "DeleteIntegration",
                    &[("IntegrationIdentifier", named)],
                )
                .await,
                404,
                "IntegrationNotFoundFault",
            );
        }
        let listed = expect_ok(
            case,
            call(&router, "DescribeIntegrations", &[("MaxRecords", "100")]).await,
        );
        assert!(
            !texts(&listed, "IntegrationName").contains(&row.name.to_string()),
            "case {case:?}: a deleted integration is in no report, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_identifier_that_names_no_integration_is_the_modelled_not_found() {
    let router = emulator();
    let source_arn = source_database(
        &router,
        &DeleteCase {
            case: "the held integration",
            name: "held-integration",
            source_is_a_cluster: true,
            by_arn: false,
        },
    )
    .await;
    let created = expect_ok(
        "create the held integration",
        call(
            &router,
            "CreateIntegration",
            &[
                ("IntegrationName", "held-integration"),
                ("SourceArn", source_arn.as_str()),
                ("TargetArn", REDSHIFT_TARGET_ARN),
            ],
        )
        .await,
    );
    let held_arn = text(&created, "IntegrationArn");
    wait_for_settle();

    // (case, the region the call is signed for, the identifier it names) — the ways an
    // identifier can name no integration the account holds: a name nothing created, an ARN
    // nothing created, and the held integration read from another region, since RDS is
    // regional.
    let rows: [(&str, &str, &str); 4] = [
        ("a name nothing created", REGION, "no-such-integration"),
        (
            "an ARN nothing created",
            REGION,
            "arn:aws:rds:us-west-2:000000000000:integration:11111111-2222-3333-4444-555555555555",
        ),
        ("the held name in another region", "eu-west-1", "held-integration"),
        ("the held ARN in another region", "eu-west-1", held_arn.as_str()),
    ];

    for (case, region, identifier) in rows {
        let body = expect_error(
            case,
            call_in(
                &router,
                region,
                "DeleteIntegration",
                &[("IntegrationIdentifier", identifier)],
            )
            .await,
            404,
            "IntegrationNotFoundFault",
        );
        // The refusal names the integration it could not find by the ARN that identifies
        // one integration of one account in one region: the value as the request spelled it
        // where that was already an ARN, and otherwise the ARN the name would have in the
        // region the call was signed for.
        let expected = if identifier.starts_with("arn:") {
            identifier.to_string()
        } else {
            format!("arn:aws:rds:{region}:{ACCOUNT}:integration:{identifier}")
        };
        assert_eq!(
            text(&body, "Message"),
            format!("Integration with identifier {expected} could not be found."),
            "case {case:?}: got {body}"
        );
    }

    // Every refusal above left the integration that IS held exactly where it was.
    let reported = expect_ok(
        "the held integration after every refusal",
        describe(&router, "held-integration").await,
    );
    assert_eq!(
        text(&reported, "IntegrationArn"),
        held_arn,
        "case \"the held integration after every refusal\": got {reported}"
    );
}
