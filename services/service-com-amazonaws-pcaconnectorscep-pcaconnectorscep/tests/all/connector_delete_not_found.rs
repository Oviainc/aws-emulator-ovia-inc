// SPDX-License-Identifier: CC0-1.0

//! `DeleteConnector` answers for the record its ARN names, not for the call being retried.
//!
//! The rule under test: an operation of this service whose input identifies a resource by its
//! ARN answers `ResourceNotFoundException` (HTTP 404) — carrying that ARN as `ResourceId` and
//! the kind of resource it names as `ResourceType` — whenever no such record is stored at the
//! instant of the call, and succeeds only when one is. `@idempotent` says a retry is safe to
//! repeat, which constrains the EFFECT of the second call (it schedules the same teardown and
//! nothing else), not the status of a call about a record that is not there.
//!
//! Every case drives the service the way a client does, one HTTP request per call through the
//! same `MultiServiceRouter` the emulator boots. The table's rows differ along what the rule
//! generalises over: HOW the ARN came to hold no record (never created, torn down from
//! `ACTIVE`, torn down from `CREATING`), whether other connectors are stored beside it, the
//! partition and region in the ARN, and the connector id. The repeat delete the issue was
//! filed about is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state;

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN of this service carries.
const ACCOUNT: &str = "000000000000";

/// The `ResourceType` a connector's `ResourceNotFoundException` names, from the service's own
/// vocabulary rather than respelled here.
const RESOURCE_TYPE_CONNECTOR: &str = state::RESOURCE_TYPE_CONNECTOR;

/// The status a connector carries while its teardown runs.
const STATUS_DELETING: &str = state::STATUS_DELETING;

/// Private-CA ARNs matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`). Rows name
/// different CAs, so no assertion can pass by echoing one recorded value.
const CA_ARNS: &[&str] = &[
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/deadbeef-0000-1111-2222-333344445555",
];

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    harness::call(router, REGION, "POST", "/connectors", Some(request)).await
}

async fn get_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn list_connectors(router: &MultiServiceRouter) -> Answer {
    harness::call(router, REGION, "GET", "/connectors", None).await
}

async fn delete_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    harness::call(
        router,
        REGION,
        "DELETE",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

/// Create a connector from `ca_arn` and answer its ARN. It starts `CREATING`.
async fn new_connector(router: &MultiServiceRouter, ca_arn: &str) -> String {
    let created = create_connector(router, json!({ "CertificateAuthorityArn": ca_arn })).await;
    assert_eq!(
        created.status, 202,
        "case {ca_arn:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    created.body["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ConnectorArn in {:?}", created.body))
        .to_string()
}

/// A connector ARN of exactly the form `ConnectorArn`'s own pattern requires
/// (`^arn:aws(-[a-z]+)*:pca-connector-scep:<region>:<account>:connector/<uuid>$`), built from
/// the parts a row varies.
fn connector_arn(partition: &str, region: &str, connector_id: &str) -> String {
    format!("arn:{partition}:pca-connector-scep:{region}:{ACCOUNT}:connector/{connector_id}")
}

/// How the ARN under test came to hold no connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Missing {
    /// Nothing was ever created under the ARN.
    NeverCreated,
    /// A connector was created, brought to `ACTIVE`, deleted, and its teardown came due.
    TornDownFromActive,
    /// A connector was deleted while the clock still owed it its `ACTIVE`, and its teardown
    /// came due.
    TornDownFromCreating,
}

/// One row: a well-formed connector ARN that holds no record, and how it got that way.
#[derive(Debug)]
struct MissingCase {
    name: &'static str,
    how: Missing,
    /// The partition in the ARN. The pattern allows `aws` and any `aws-<suffix>` form, so a
    /// row can vary it; only rows that create a record use the emulator's own.
    partition: &'static str,
    /// The region in the ARN, likewise.
    region: &'static str,
    /// The connector uuid for the rows that build the ARN by hand.
    connector_id: &'static str,
    /// How many OTHER connectors are stored when the call arrives, so the refusal cannot come
    /// from an empty store.
    siblings: usize,
}

/// The whole rule, row by row: every well-formed connector ARN that holds no record is a 404
/// `ResourceNotFoundException` naming that ARN and the `Connector` resource type.
#[tokio::test]
async fn delete_connector_refuses_an_arn_that_holds_no_connector() {
    let cases = [
        MissingCase {
            name: "never created, empty store",
            how: Missing::NeverCreated,
            partition: "aws",
            region: REGION,
            connector_id: "11111111-1111-1111-1111-111111111111",
            siblings: 0,
        },
        MissingCase {
            name: "never created, other connectors stored",
            how: Missing::NeverCreated,
            partition: "aws",
            region: REGION,
            connector_id: "ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb",
            siblings: 2,
        },
        MissingCase {
            name: "never created, another partition and region",
            how: Missing::NeverCreated,
            partition: "aws-us-gov",
            region: "us-gov-west-1",
            connector_id: "0123456f-89ab-cdef-0123-456789abcdef",
            siblings: 1,
        },
        MissingCase {
            name: "torn down from ACTIVE",
            how: Missing::TornDownFromActive,
            partition: "aws",
            region: REGION,
            connector_id: "",
            siblings: 0,
        },
        MissingCase {
            name: "torn down from ACTIVE, other connectors stored",
            how: Missing::TornDownFromActive,
            partition: "aws",
            region: REGION,
            connector_id: "",
            siblings: 3,
        },
        MissingCase {
            name: "torn down from CREATING",
            how: Missing::TornDownFromCreating,
            partition: "aws",
            region: REGION,
            connector_id: "",
            siblings: 1,
        },
    ];

    for case in &cases {
        let name = case.name;
        let (router, engine) = new_router();

        let mut siblings = Vec::new();
        for index in 0..case.siblings {
            let arn = new_connector(&router, CA_ARNS[index % CA_ARNS.len()]).await;
            harness::release_pending(&engine, &arn);
            siblings.push(arn);
        }

        let arn = match case.how {
            Missing::NeverCreated => {
                connector_arn(case.partition, case.region, case.connector_id)
            }
            Missing::TornDownFromActive | Missing::TornDownFromCreating => {
                let arn = new_connector(&router, CA_ARNS[case.siblings % CA_ARNS.len()]).await;
                if case.how == Missing::TornDownFromActive {
                    harness::release_pending(&engine, &arn);
                } else {
                    harness::hold_pending(&engine, &arn);
                }
                let accepted = delete_connector(&router, &arn).await;
                assert_eq!(
                    (accepted.status, accepted.error_type.as_str()),
                    (202, ""),
                    "case {name:?}: the first DeleteConnector must be accepted, got {} / {:?}",
                    accepted.status,
                    accepted.body
                );
                // Let the clock make the teardown due: only then is the record gone.
                harness::release_pending(&engine, &arn);
                let read = get_connector(&router, &arn).await;
                assert_eq!(
                    read.status, 404,
                    "case {name:?}: the teardown must remove the record, got {} / {:?}",
                    read.status, read.body
                );
                arn
            }
        };

        let answer = delete_connector(&router, &arn).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {name:?} on {arn:?}: expected 404 ResourceNotFoundException, got {} / {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["ResourceId"],
            json!(arn),
            "case {name:?}: the refusal must name the ARN it was asked for, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["ResourceType"],
            json!(RESOURCE_TYPE_CONNECTOR),
            "case {name:?}: expected ResourceType {RESOURCE_TYPE_CONNECTOR}, got {:?}",
            answer.body
        );

        // The refusal changed nothing: it created no record, it left every sibling readable,
        // and a second attempt answers exactly the same way, so the 404 reports the state
        // rather than being a one-shot.
        let still_gone = get_connector(&router, &arn).await;
        assert_eq!(
            still_gone.status, 404,
            "case {name:?}: the refused delete must not create a record, got {} / {:?}",
            still_gone.status, still_gone.body
        );
        let repeat = delete_connector(&router, &arn).await;
        assert_eq!(
            (repeat.status, repeat.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {name:?}: the refusal must be stable across calls, got {} / {:?}",
            repeat.status,
            repeat.body
        );
        for sibling in &siblings {
            let read = get_connector(&router, sibling).await;
            assert_eq!(
                read.status, 200,
                "case {name:?}: the refusal must leave sibling {sibling:?} readable, got {} / \
                 {:?}",
                read.status, read.body
            );
        }
    }
}

/// The success side of the same rule: a delete of an ARN that DOES hold a record is accepted,
/// stays accepted for as long as the record is there (the `@idempotent` retry), removes
/// exactly that connector, and only then starts answering 404.
#[tokio::test]
async fn delete_connector_is_accepted_while_the_record_is_there() {
    // Rows differ in how many connectors are stored and which of them is deleted: the first,
    // a middle one and the last.
    for (count, victim) in [(1usize, 0usize), (2, 1), (3, 1), (4, 0), (4, 3)] {
        let case = format!("{count} connectors, deleting #{victim}");
        let (router, engine) = new_router();

        let mut arns = Vec::new();
        for index in 0..count {
            let arn = new_connector(&router, CA_ARNS[index % CA_ARNS.len()]).await;
            harness::release_pending(&engine, &arn);
            arns.push(arn);
        }

        let doomed = arns[victim].clone();
        let accepted = delete_connector(&router, &doomed).await;
        assert_eq!(
            (accepted.status, accepted.error_type.as_str()),
            (202, ""),
            "case {case:?}: a delete of a stored connector must be accepted, got {} / {:?}",
            accepted.status,
            accepted.body
        );

        // Pin the teardown window: while the record is still there the retry is accepted too,
        // and it reports the same status rather than a fresh one.
        harness::hold_pending(&engine, &doomed);
        let again = delete_connector(&router, &doomed).await;
        assert_eq!(
            (again.status, again.error_type.as_str()),
            (202, ""),
            "case {case:?}: a retry inside the teardown window must still be accepted, got {} / \
             {:?}",
            again.status,
            again.body
        );
        let reported = get_connector(&router, &doomed).await;
        assert_eq!(
            reported.body["Connector"]["Status"],
            json!(STATUS_DELETING),
            "case {case:?}: the connector under teardown must report {STATUS_DELETING}, got {:?}",
            reported.body
        );

        // Once the teardown is due the record is gone and the same call is refused.
        harness::release_pending(&engine, &doomed);
        let refused = delete_connector(&router, &doomed).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: once the record is gone the delete must be refused, got {} / {:?}",
            refused.status,
            refused.body
        );

        // Exactly that connector went: every other one is still listed and still deletable.
        let listed = list_connectors(&router).await;
        let listed_arns: Vec<String> = listed.body["Connectors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Connectors array in {:?}", listed.body))
            .iter()
            .filter_map(|entry| entry["Arn"].as_str().map(str::to_string))
            .collect();
        for (index, arn) in arns.iter().enumerate() {
            if index == victim {
                assert!(
                    !listed_arns.contains(arn),
                    "case {case:?}: the deleted connector must not be listed, got {listed_arns:?}"
                );
                continue;
            }
            assert!(
                listed_arns.contains(arn),
                "case {case:?}: sibling {arn:?} must still be listed, got {listed_arns:?}"
            );
            let accepted = delete_connector(&router, arn).await;
            assert_eq!(
                (accepted.status, accepted.error_type.as_str()),
                (202, ""),
                "case {case:?}: sibling {arn:?} must still be deletable, got {} / {:?}",
                accepted.status,
                accepted.body
            );
        }
    }
}
