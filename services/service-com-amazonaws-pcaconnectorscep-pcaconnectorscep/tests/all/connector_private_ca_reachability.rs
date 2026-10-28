// SPDX-License-Identifier: CC0-1.0

//! A connector comes up only when the private CA its create names is one it could reach.
//!
//! The rule these cases assert, over a table that spans the class rather than one request: a
//! connector reaches its private CA as the account it is created in, from the Region it is
//! created in, so the partition, Region and account segments of `CertificateAuthorityArn`
//! must be the ones the connector's own ARN carries. A create naming a CA outside that reach
//! is still ACCEPTED — `CreateConnector` answers 202 and the connector starts `CREATING` — and
//! the outcome of linking the CA is reported on the connector: it settles into `FAILED`
//! carrying the `ConnectorStatusReason` for that cause instead of into `ACTIVE`.
//!
//! Why the outcome is reported on the connector rather than refused at the call:
//! `CreateConnector` is asynchronous (its modelled success is HTTP 202 and `ConnectorStatus`
//! carries `CREATING`), and `StatusReason` is documented as "Information about why connector
//! creation failed, if status is `FAILED`" (smithy_query shape=Connector member=StatusReason)
//! over an enum whose values include the private-CA ones (smithy_query
//! shape=ConnectorStatusReason) — values only a connector that was created and then failed can
//! report.
//!
//! Rows differ along what the rule generalises over: which of the three placing segments
//! differs (partition, Region, account), how far it differs (an adjacent Region name, a Region
//! differing only in its number, an account id differing in one digit, a partition with one or
//! three words), how many differ at once, and which CA a reachable row names — so no assertion
//! can pass by echoing one value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

/// The Region every case in this suite is signed for — not the emulator default, so nothing
/// can pass by a hardcoded default.
const REGION: &str = "eu-west-2";

/// A second Region, used where a case needs one this service also serves.
const OTHER_REGION: &str = "ap-northeast-1";

/// The account every ARN this emulator mints carries, taken from the runtime rather than
/// respelled, so a row that names "another account" is one this emulator really does not
/// serve.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The status a freshly created connector is in until its transition comes due.
const STATUS_CREATING: &str = scep::state::STATUS_CREATING;
/// The settled status of a connector whose CA it could reach.
const STATUS_ACTIVE: &str = scep::state::STATUS_ACTIVE;
/// The settled status of a connector whose creation did not come up.
const STATUS_FAILED: &str = scep::state::STATUS_FAILED;

/// `ConnectorStatusReason` of a connector whose private CA could not be found.
const REASON_NOT_FOUND: &str = scep::state::REASON_PRIVATECA_RESOURCE_NOT_FOUND;
/// `ConnectorStatusReason` of a connector not allowed to use the CA it names.
const REASON_ACCESS_DENIED: &str = scep::state::REASON_PRIVATECA_ACCESS_DENIED;

/// Every value the model declares for `ConnectorStatus` / `ConnectorStatusReason`, so a row's
/// expectation is checked to be a value the wire enum accepts and not a string of this
/// suite's own (smithy_query shape=ConnectorStatus,ConnectorStatusReason).
const CONNECTOR_STATUS_VALUES: &[&str] =
    scep::ops::types::shared_types::CONNECTOR_STATUS_VALUES;
/// The `ConnectorStatusReason` catalogue, from the same place.
const CONNECTOR_STATUS_REASON_VALUES: &[&str] =
    scep::ops::types::shared_types::CONNECTOR_STATUS_REASON_VALUES;

/// A private-CA ARN in the form `CertificateAuthorityArn`'s own `@pattern` requires:
/// `^arn:aws(-[a-z]+)*:acm-pca:<region>:<12 digits>:certificate-authority/<uuid>$`. Every row
/// builds its ARN here, so each row differs only in the segments the rule is about.
fn ca_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:acm-pca:{region}:{account}:certificate-authority/{id}")
}

/// One row: the CA a create names, and the `ConnectorStatusReason` the connector must settle
/// with — `None` where the CA is one the connector could reach, which settles it `ACTIVE`
/// with no reason at all.
#[derive(Debug)]
struct Row {
    name: &'static str,
    ca_arn: String,
    reason: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    let reachable = |name: &'static str, id: &str| Row {
        name,
        ca_arn: ca_arn("aws", REGION, ACCOUNT, id),
        reason: None,
    };
    vec![
        reachable(
            "a CA of this connector's own partition, Region and account",
            "11111111-2222-3333-4444-555555555555",
        ),
        reachable(
            "another CA of the same partition, Region and account",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        ),
        Row {
            name: "a CA one Region away",
            ca_arn: ca_arn(
                "aws",
                OTHER_REGION,
                ACCOUNT,
                "0123456f-89ab-cdef-0123-456789abcdef",
            ),
            reason: Some(REASON_NOT_FOUND),
        },
        Row {
            name: "a CA in the Region this emulator serves by default",
            ca_arn: ca_arn(
                "aws",
                scep::state::DEFAULT_REGION,
                ACCOUNT,
                "deadbeef-0000-1111-2222-333344445555",
            ),
            reason: Some(REASON_NOT_FOUND),
        },
        Row {
            name: "a CA whose Region name differs only in its number",
            ca_arn: ca_arn(
                "aws",
                "eu-west-3",
                ACCOUNT,
                "99999999-8888-7777-6666-555544443333",
            ),
            reason: Some(REASON_NOT_FOUND),
        },
        Row {
            name: "a CA of the same Region and account in another partition",
            ca_arn: ca_arn(
                "aws-cn",
                REGION,
                ACCOUNT,
                "abcdef01-2345-6789-abcd-ef0123456789",
            ),
            reason: Some(REASON_NOT_FOUND),
        },
        Row {
            name: "a CA in a partition whose name carries three words",
            ca_arn: ca_arn(
                "aws-us-gov",
                REGION,
                ACCOUNT,
                "12345678-90ab-cdef-1234-567890abcdef",
            ),
            reason: Some(REASON_NOT_FOUND),
        },
        Row {
            name: "a CA of another account in this connector's own Region",
            ca_arn: ca_arn(
                "aws",
                REGION,
                "123456789012",
                "22222222-3333-4444-5555-666677778888",
            ),
            reason: Some(REASON_ACCESS_DENIED),
        },
        Row {
            name: "a CA of an account whose id differs in one digit",
            ca_arn: ca_arn(
                "aws",
                REGION,
                "000000000001",
                "33333333-4444-5555-6666-777788889999",
            ),
            reason: Some(REASON_ACCESS_DENIED),
        },
        Row {
            // Both segments differ: the CA is not in a Region this endpoint resolves at all,
            // so it is the one that cannot be found rather than the one about permission.
            name: "a CA of another account in another Region",
            ca_arn: ca_arn(
                "aws",
                OTHER_REGION,
                "123456789012",
                "44444444-5555-6666-7777-888899990000",
            ),
            reason: Some(REASON_NOT_FOUND),
        },
    ]
}

async fn create_connector(router: &MultiServiceRouter, body: Value) -> harness::Answer {
    harness::call(router, REGION, "POST", "/connectors", Some(body)).await
}

async fn get_connector(router: &MultiServiceRouter, arn: &str) -> harness::Answer {
    harness::call(
        router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

/// The `ConnectorSummary` `ListConnectors` reports for `arn`.
async fn listed_connector(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let page = harness::call(router, REGION, "GET", "/connectors", None).await;
    assert_eq!(
        page.status, 200,
        "case {case:?}: expected 200 from ListConnectors, got {} / {:?}",
        page.status, page.body
    );
    page.body["Connectors"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["Arn"] == json!(arn))
        .unwrap_or_else(|| panic!("case {case:?}: no ConnectorSummary for {arn:?} in {:?}", page.body))
        .clone()
}

/// Create the row's connector, holding its transition off so the transient status is
/// observable, and answer its ARN.
async fn created_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    ca_arn: &str,
) -> String {
    let created = create_connector(router, json!({ "CertificateAuthorityArn": ca_arn })).await;
    assert_eq!(
        (created.status, created.error_type.as_str()),
        (202, ""),
        "case {case:?}: a create naming {ca_arn:?} is accepted, so expected 202, got {} / {:?}",
        created.status,
        created.body
    );
    let arn = created.text("ConnectorArn");
    harness::hold_pending(engine, &arn);
    arn
}

/// The whole rule, on both readers of the record: whatever the CA ARN a create names, the
/// connector starts `CREATING` with no reason, and settles into the status and
/// `ConnectorStatusReason` that CA's reachability decides.
#[tokio::test]
async fn a_connector_settles_by_whether_its_private_ca_is_one_it_could_reach() {
    for row in rows() {
        let case = row.name;
        let (router, engine) = new_router();

        let expected_status = if row.reason.is_some() {
            STATUS_FAILED
        } else {
            STATUS_ACTIVE
        };
        assert!(
            CONNECTOR_STATUS_VALUES.contains(&expected_status),
            "case {case:?}: {expected_status:?} must be a declared ConnectorStatus, got \
             {CONNECTOR_STATUS_VALUES:?}"
        );
        if let Some(reason) = row.reason {
            assert!(
                CONNECTOR_STATUS_REASON_VALUES.contains(&reason),
                "case {case:?}: {reason:?} must be a declared ConnectorStatusReason, got \
                 {CONNECTOR_STATUS_REASON_VALUES:?}"
            );
        }

        let arn = created_connector(&router, &engine, case, &row.ca_arn).await;

        // Before the transition is due every connector reads the same: still provisioning,
        // with nothing said about why anything failed.
        let creating = get_connector(&router, &arn).await;
        assert_eq!(
            creating.connector("Status"),
            &json!(STATUS_CREATING),
            "case {case:?}: a connector whose transition is not due yet is {STATUS_CREATING}, \
             got {:?}",
            creating.body
        );
        assert_eq!(
            creating.connector("StatusReason"),
            &Value::Null,
            "case {case:?}: a connector that has not failed reports no StatusReason, got {:?}",
            creating.body
        );

        harness::release_pending(&engine, &arn);

        let settled = get_connector(&router, &arn).await;
        assert_eq!(
            settled.connector("Status"),
            &json!(expected_status),
            "case {case:?}: a connector linking {:?} settles into {expected_status}, got {:?}",
            row.ca_arn,
            settled.body
        );
        let expected_reason = row.reason.map_or(Value::Null, |reason| json!(reason));
        assert_eq!(
            settled.connector("StatusReason"),
            &expected_reason,
            "case {case:?}: expected StatusReason {expected_reason:?}, got {:?}",
            settled.body
        );
        // The record is still the one the request asked for: the CA ARN is stored and
        // reported exactly as it was named, whatever the outcome of linking it was.
        assert_eq!(
            settled.connector("CertificateAuthorityArn"),
            &json!(row.ca_arn),
            "case {case:?}: expected the CA ARN the create named, got {:?}",
            settled.body
        );

        // Both readers project one record, so the listing agrees with the single read.
        let listed = listed_connector(&router, case, &arn).await;
        assert_eq!(
            (&listed["Status"], &listed["StatusReason"]),
            (settled.connector("Status"), settled.connector("StatusReason")),
            "case {case:?}: ListConnectors must report what GetConnector does, got {listed:?} \
             against {:?}",
            settled.body
        );
    }
}

/// A connector that never came up serves nothing: it cannot mint a challenge password, while
/// one whose CA it could reach can. Both are then deletable, so a failed creation leaves no
/// record a caller cannot get rid of.
#[tokio::test]
async fn only_a_connector_that_came_up_serves_challenges_and_both_can_be_deleted() {
    for row in rows() {
        let case = row.name;
        let (router, engine) = new_router();
        let arn = created_connector(&router, &engine, case, &row.ca_arn).await;
        harness::release_pending(&engine, &arn);

        let challenge = harness::call(
            &router,
            REGION,
            "POST",
            "/challenges",
            Some(json!({ "ConnectorArn": arn })),
        )
        .await;
        if row.reason.is_some() {
            assert_eq!(
                (challenge.status, challenge.error_type.as_str()),
                (400, "ValidationException"),
                "case {case:?}: a connector in {STATUS_FAILED} cannot mint a challenge, so \
                 expected 400 ValidationException, got {} / {:?}",
                challenge.status,
                challenge.body
            );
            assert!(
                challenge.message().contains(STATUS_FAILED),
                "case {case:?}: the refusal must name the status the connector is in, got {:?}",
                challenge.message()
            );
        } else {
            assert_eq!(
                (challenge.status, challenge.error_type.as_str()),
                (202, ""),
                "case {case:?}: a connector in {STATUS_ACTIVE} mints a challenge, got {} / {:?}",
                challenge.status,
                challenge.body
            );
        }

        let deleted = harness::call(
            &router,
            REGION,
            "DELETE",
            &format!("/connectors/{}", label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            (deleted.status, deleted.error_type.as_str()),
            (202, ""),
            "case {case:?}: the connector must be deletable, got {} / {:?}",
            deleted.status,
            deleted.body
        );
        harness::release_pending(&engine, &arn);
        let gone = get_connector(&router, &arn).await;
        assert_eq!(
            (gone.status, gone.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: once the teardown came due the record is gone, got {} / {:?}",
            gone.status,
            gone.body
        );
    }
}

/// The Region the rule compares against is the one the connector is CREATED in — the Region
/// the request's credential scope names, which is the Region its own ARN carries — not the
/// endpoint host the call was addressed to.
#[tokio::test]
async fn the_region_compared_against_is_the_one_the_connector_is_created_in() {
    // (which Region the CA ARN names, the reason the connector must settle with)
    let cases: [(&str, &str, Option<&str>); 2] = [
        (
            "the CA is in the Region the credential scope names",
            OTHER_REGION,
            None,
        ),
        (
            "the CA is in the Region the request was addressed to",
            REGION,
            Some(REASON_NOT_FOUND),
        ),
    ];

    for (case, ca_region, reason) in cases {
        let (router, engine) = new_router();
        // Addressed at REGION's endpoint, signed for OTHER_REGION: the record, and so the
        // connector, is created in OTHER_REGION.
        let created = harness::call_with_scope(
            &router,
            REGION,
            OTHER_REGION,
            "POST",
            "/connectors",
            Some(json!({
                "CertificateAuthorityArn": ca_arn(
                    "aws",
                    ca_region,
                    ACCOUNT,
                    "55555555-6666-7777-8888-99990000aaaa",
                ),
            })),
        )
        .await;
        assert_eq!(
            (created.status, created.error_type.as_str()),
            (202, ""),
            "case {case:?}: expected 202 from CreateConnector, got {} / {:?}",
            created.status,
            created.body
        );
        let arn = created.text("ConnectorArn");
        assert!(
            arn.contains(&format!(":{OTHER_REGION}:")),
            "case {case:?}: the connector is created in the scope Region {OTHER_REGION}, got \
             {arn:?}"
        );
        harness::release_pending(&engine, &arn);

        let read = harness::call(
            &router,
            OTHER_REGION,
            "GET",
            &format!("/connectors/{}", label(&arn)),
            None,
        )
        .await;
        let expected_status = if reason.is_some() {
            STATUS_FAILED
        } else {
            STATUS_ACTIVE
        };
        assert_eq!(
            read.connector("Status"),
            &json!(expected_status),
            "case {case:?}: expected {expected_status}, got {:?}",
            read.body
        );
        assert_eq!(
            read.connector("StatusReason"),
            &reason.map_or(Value::Null, |reason| json!(reason)),
            "case {case:?}: expected StatusReason {reason:?}, got {:?}",
            read.body
        );
    }
}

/// A retry is still a retry: the same `ClientToken` on a create whose CA it could not reach
/// answers the connector that create already made, rather than filing a second failed one.
#[tokio::test]
async fn a_repeated_client_token_answers_the_connector_it_already_created() {
    let (router, engine) = new_router();
    let case = "a repeated token on a create whose CA cannot be reached";
    let request = json!({
        "CertificateAuthorityArn": ca_arn(
            "aws",
            OTHER_REGION,
            ACCOUNT,
            "66666666-7777-8888-9999-0000aaaabbbb",
        ),
        "ClientToken": "a-retry-token-0001",
    });

    let first = create_connector(&router, request.clone()).await;
    assert_eq!(
        (first.status, first.error_type.as_str()),
        (202, ""),
        "case {case:?}: expected 202 from CreateConnector, got {} / {:?}",
        first.status,
        first.body
    );
    let arn = first.text("ConnectorArn");
    harness::release_pending(&engine, &arn);

    let retried = create_connector(&router, request).await;
    assert_eq!(
        retried.text("ConnectorArn"),
        arn,
        "case {case:?}: the token must answer with the connector it created, got {:?}",
        retried.body
    );
    let listed = harness::call(&router, REGION, "GET", "/connectors", None).await;
    assert_eq!(
        listed.count("Connectors"),
        1,
        "case {case:?}: the retry must not file a second connector, got {:?}",
        listed.body
    );
    let read = get_connector(&router, &arn).await;
    assert_eq!(
        (read.connector("Status"), read.connector("StatusReason")),
        (&json!(STATUS_FAILED), &json!(REASON_NOT_FOUND)),
        "case {case:?}: the one connector the token stands for reports why it failed, got {:?}",
        read.body
    );
}
