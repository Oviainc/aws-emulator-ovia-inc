// SPDX-License-Identifier: CC0-1.0

//! What a created Aurora DSQL change data capture (CDC) stream settles into, end
//! to end over the wire.
//!
//! RULE: a stream's delivery target being unusable is stream STATE, not a refusal
//! of the create — so `CreateStream` is served, and the clock settles the stream
//! into `ACTIVE` only when the services that OWN its `targetDefinition` answered
//! for it. When one of them answers that it does not, the stream settles into
//! `FAILED` and `GetStream` reports a `statusReason` whose `error` is the
//! `StreamFailureErrorCode` that answer names, beside the instant the status was
//! reached. A target nothing answered about is left to settle `ACTIVE`: a failure
//! has to be grounded in what the owner of the target said about it.
//!
//! The emulator here holds Aurora DSQL beside the two services a target names —
//! Amazon Kinesis, which owns the data stream it addresses, and IAM, which holds
//! the role it asks this service to assume — on one engine, so every row's target
//! is resolved against the very streams and roles the row created through those
//! services' own APIs. Every call goes the whole way round through
//! `MultiServiceRouter::dispatch`.
//!
//! The table asserts the rule over a CLASS of targets rather than one request: a
//! destination the delivery service holds, one it never held, one written for
//! another Region and one written for another account; a role whose trust policy
//! admits this service, one admitting another service, one under a path, and one
//! IAM holds nothing under — and a row where BOTH halves are unusable, which pins
//! which of them the stream reports.

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::STREAM_FAILURE_ERROR_CODE_VALUES;
use service_com_amazonaws_dsql_dsql::state::{
    STREAM_FAILURE_KINESIS_STREAM_NOT_FOUND, STREAM_FAILURE_ROLE_ACCESS_DENIED,
    STREAM_STATUS_ACTIVE, STREAM_STATUS_CREATING, STREAM_STATUS_DELETING, STREAM_STATUS_FAILED,
};

use crate::harness::{wait_for_the_clock, xml_text, Answer, Dsql};

/// The Region every row below is signed for.
const REGION: &str = "us-east-1";

/// A Region that is NOT [`REGION`], for the row whose destination ARN names
/// another Region than the endpoint that holds the stream.
const ANOTHER_REGION: &str = "eu-central-1";

/// An account that is not this emulator's, for the row whose destination ARN names
/// another account's stream. Twelve digits, as every ARN's account field is.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// The service principal a role has to admit for Aurora DSQL to deliver under it.
const THIS_SERVICE_PRINCIPAL: &str = "dsql.amazonaws.com";

/// A principal that is not this service's, for the rows whose role admits somebody
/// else.
const ANOTHER_SERVICE_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The data stream the delivery service holds for the rows that name a held
/// destination. `StreamName` is `^[a-zA-Z0-9_.-]+$` (Kinesis model).
const HELD_DELIVERY_STREAM: &str = "dsql-changes";

/// A data stream name nothing ever created.
const ABSENT_DELIVERY_STREAM: &str = "never-created";

/// How a row writes the `streamArn` of its target.
#[derive(Debug, Clone, Copy)]
enum Destination {
    /// The ARN the delivery service itself answered for the stream this suite
    /// created there — so the value under test is one that service minted.
    Held,
    /// A well-formed ARN, of this account and this Region, of a stream nothing
    /// created.
    Absent,
    /// The held stream's name, written as an ARN of another Region: that Region's
    /// endpoint holds no such stream.
    HeldButAnotherRegion,
    /// The held stream's name, written as an ARN of another account: no endpoint
    /// here holds a stream under that ARN.
    HeldButAnotherAccount,
}

/// How a row writes the `roleArn` of its target.
#[derive(Debug, Clone, Copy)]
enum Pass {
    /// A role IAM holds whose trust policy admits this service.
    TrustingThisService,
    /// The same, under an IAM path (`:role/service-role/<name>`), the form the
    /// model documents for a console-created role.
    TrustingThisServiceUnderAPath,
    /// A role IAM holds whose trust policy admits another service only.
    TrustingAnotherService,
    /// A well-formed role ARN of this account that IAM holds nothing under.
    IamHoldsNoSuchRole,
}

/// One target, and what the stream created against it must settle into.
struct TargetCase {
    name: &'static str,
    destination: Destination,
    pass: Pass,
    /// The `StreamStatus` the clock must settle the stream into.
    settles_into: &'static str,
    /// The `statusReason.error` a read must then report, or `None` when the read
    /// must report no `statusReason` at all.
    reason: Option<&'static str>,
}

fn target_cases() -> Vec<TargetCase> {
    vec![
        TargetCase {
            name: "a destination the delivery service holds, a role trusting this service",
            destination: Destination::Held,
            pass: Pass::TrustingThisService,
            settles_into: STREAM_STATUS_ACTIVE,
            reason: None,
        },
        TargetCase {
            name: "the same destination, the trusting role written under a path",
            destination: Destination::Held,
            pass: Pass::TrustingThisServiceUnderAPath,
            settles_into: STREAM_STATUS_ACTIVE,
            reason: None,
        },
        TargetCase {
            name: "the same destination, a role IAM holds nothing under",
            destination: Destination::Held,
            pass: Pass::IamHoldsNoSuchRole,
            settles_into: STREAM_STATUS_ACTIVE,
            reason: None,
        },
        TargetCase {
            name: "a destination nothing ever created",
            destination: Destination::Absent,
            pass: Pass::TrustingThisService,
            settles_into: STREAM_STATUS_FAILED,
            reason: Some(STREAM_FAILURE_KINESIS_STREAM_NOT_FOUND),
        },
        TargetCase {
            name: "a held destination written for another Region",
            destination: Destination::HeldButAnotherRegion,
            pass: Pass::TrustingThisService,
            settles_into: STREAM_STATUS_FAILED,
            reason: Some(STREAM_FAILURE_KINESIS_STREAM_NOT_FOUND),
        },
        TargetCase {
            name: "a held destination written for another account",
            destination: Destination::HeldButAnotherAccount,
            pass: Pass::TrustingThisService,
            settles_into: STREAM_STATUS_FAILED,
            reason: Some(STREAM_FAILURE_KINESIS_STREAM_NOT_FOUND),
        },
        TargetCase {
            name: "a held destination, a role trusting another service",
            destination: Destination::Held,
            pass: Pass::TrustingAnotherService,
            settles_into: STREAM_STATUS_FAILED,
            reason: Some(STREAM_FAILURE_ROLE_ACCESS_DENIED),
        },
        TargetCase {
            name: "neither half usable: the role is what the stream reports",
            destination: Destination::Absent,
            pass: Pass::TrustingAnotherService,
            settles_into: STREAM_STATUS_FAILED,
            reason: Some(STREAM_FAILURE_ROLE_ACCESS_DENIED),
        },
    ]
}

/// The two services a target names, holding what the table's rows address: one
/// data stream and three roles, each created through that service's own API and
/// each identified by the ARN that service answered with.
struct Targets {
    /// The ARN Amazon Kinesis minted for [`HELD_DELIVERY_STREAM`].
    held_stream_arn: String,
    trusting_role_arn: String,
    trusting_role_under_a_path_arn: String,
    role_trusting_another_service_arn: String,
    /// A role ARN of this account IAM holds nothing under, written in the account
    /// the held role's own ARN carries.
    unheld_role_arn: String,
}

impl Targets {
    /// The `streamArn` a row's destination is written as.
    fn stream_arn(&self, destination: Destination) -> String {
        match destination {
            Destination::Held => self.held_stream_arn.clone(),
            Destination::Absent => kinesis_arn(
                REGION,
                &account_of(&self.held_stream_arn),
                ABSENT_DELIVERY_STREAM,
            ),
            Destination::HeldButAnotherRegion => kinesis_arn(
                ANOTHER_REGION,
                &account_of(&self.held_stream_arn),
                HELD_DELIVERY_STREAM,
            ),
            Destination::HeldButAnotherAccount => {
                kinesis_arn(REGION, ANOTHER_ACCOUNT, HELD_DELIVERY_STREAM)
            }
        }
    }

    /// The `roleArn` a row's pass is written as.
    fn role_arn(&self, pass: Pass) -> String {
        match pass {
            Pass::TrustingThisService => self.trusting_role_arn.clone(),
            Pass::TrustingThisServiceUnderAPath => self.trusting_role_under_a_path_arn.clone(),
            Pass::TrustingAnotherService => self.role_trusting_another_service_arn.clone(),
            Pass::IamHoldsNoSuchRole => self.unheld_role_arn.clone(),
        }
    }

    /// The `targetDefinition` a row sends.
    fn definition(&self, case: &TargetCase) -> Value {
        json!({"kinesis": {
            "streamArn": self.stream_arn(case.destination),
            "roleArn": self.role_arn(case.pass),
        }})
    }
}

/// A Kinesis data stream ARN, as `KinesisStreamArn`'s pattern requires
/// (`^arn:aws[a-zA-Z-]*:kinesis:[a-z0-9-]*:[0-9]{12}:stream/[a-zA-Z0-9+=,.@_/\-]+$`,
/// `smithy_query(query="shape", shape="KinesisTargetDefinition")`).
fn kinesis_arn(region: &str, account: &str, name: &str) -> String {
    format!("arn:aws:kinesis:{region}:{account}:stream/{name}")
}

/// The account field of an ARN — read off an ARN a service itself minted rather
/// than written as a literal.
fn account_of(arn: &str) -> String {
    arn.split(':').nth(4).unwrap_or_default().to_string()
}

/// Everything the table addresses, created through the owning services' own APIs.
async fn given_targets(dsql: &Dsql) -> Targets {
    let held_stream_arn = given_delivery_stream(dsql, HELD_DELIVERY_STREAM).await;
    let trusting_role_arn =
        given_role(dsql, "/", "dsql-cdc-trusted", THIS_SERVICE_PRINCIPAL).await;
    let trusting_role_under_a_path_arn =
        given_role(dsql, "/service-role/", "dsql-cdc-path", THIS_SERVICE_PRINCIPAL).await;
    let role_trusting_another_service_arn =
        given_role(dsql, "/", "dsql-cdc-elsewhere", ANOTHER_SERVICE_PRINCIPAL).await;
    let unheld_role_arn = format!(
        "arn:aws:iam::{}:role/dsql-cdc-never-created",
        account_of(&trusting_role_arn)
    );
    Targets {
        held_stream_arn,
        trusting_role_arn,
        trusting_role_under_a_path_arn,
        role_trusting_another_service_arn,
        unheld_role_arn,
    }
}

/// One Kinesis data stream, created through that service's own `CreateStream`, and
/// the `StreamARN` it answered for it.
async fn given_delivery_stream(dsql: &Dsql, name: &str) -> String {
    let created = dsql
        .rpc(
            "kinesis",
            "Kinesis_20131202",
            "CreateStream",
            json!({"StreamName": name, "ShardCount": 1}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the delivery stream {name} must be created first, got {} {:?}",
        created.status, created.body
    );
    let described = dsql
        .rpc(
            "kinesis",
            "Kinesis_20131202",
            "DescribeStreamSummary",
            json!({"StreamName": name}),
        )
        .await;
    assert_eq!(
        described.status, 200,
        "the delivery stream {name} must be readable, got {} {:?}",
        described.status, described.body
    );
    described.body["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeStreamSummary must answer a StreamARN, got {described:?}", described = described.body))
        .to_string()
}

/// One IAM role trusting `principal`, created through IAM's own `CreateRole`, and
/// the ARN IAM answered for it.
async fn given_role(dsql: &Dsql, path: &str, name: &str, principal: &str) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": principal},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, document) = dsql
        .iam(
            "CreateRole",
            &[
                ("RoleName", name),
                ("Path", path),
                ("AssumeRolePolicyDocument", &trust),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "the role {name} must be created in IAM first, got {status} {document}"
    );
    xml_text(&document, "Arn")
        .unwrap_or_else(|| panic!("CreateRole must answer the role's ARN, got {document}"))
}

/// `GetStream`'s `statusReason`, as `(error, updatedAt)`, or `None` when the
/// answer carries none.
fn status_reason(answer: &Answer) -> Option<(String, f64)> {
    let reason = answer.get("statusReason");
    let error = reason.get("error")?.as_str()?.to_string();
    let updated_at = reason.get("updatedAt")?.as_f64()?;
    Some((error, updated_at))
}

/// RULE: the clock settles a new stream into `ACTIVE` only when the services that
/// own its `targetDefinition` answered for it; where one of them answered that it
/// does not hold the target, or that the role does not admit this service, the
/// stream settles into `FAILED` and reports the matching `StreamFailureErrorCode`
/// as its `statusReason`.
#[tokio::test]
async fn a_stream_settles_into_the_status_its_targets_owners_answered_for() {
    let dsql = Dsql::with_delivery_services(REGION);
    let targets = given_targets(&dsql).await;
    let cases = target_cases();
    let clusters = dsql.create_settled_many(cases.len()).await;

    // Every stream is created before the ONE wait, so the suite waits for the
    // clock once rather than once per row.
    let mut streams = Vec::with_capacity(cases.len());
    for (case, cluster) in cases.iter().zip(&clusters) {
        let created = dsql
            .create_stream(
                cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": targets.definition(case),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: an unusable target is stream state, not a refusal of the create, got {} {:?}",
            case.name, created.status, created.body
        );
        assert_eq!(
            created.str("status"),
            STREAM_STATUS_CREATING,
            "case {:?}: CreateStream must answer CREATING, got {:?}",
            case.name,
            created.str("status")
        );
        streams.push(created.str("streamIdentifier"));
    }

    wait_for_the_clock().await;

    for ((case, cluster), stream) in cases.iter().zip(&clusters).zip(&streams) {
        let read = dsql.get_stream(cluster, stream).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: GetStream must serve the stream, got {} {:?}",
            case.name, read.status, read.body
        );
        assert_eq!(
            read.str("status"),
            case.settles_into,
            "case {:?}: expected the clock to settle the stream into {}, got {:?}",
            case.name,
            case.settles_into,
            read.str("status")
        );
        let reported = status_reason(&read);
        match case.reason {
            None => assert!(
                reported.is_none(),
                "case {:?}: a stream whose target was answered for reports no statusReason, got {:?}",
                case.name,
                reported
            ),
            Some(expected) => {
                let (error, updated_at) = reported.unwrap_or_else(|| {
                    panic!(
                        "case {:?}: a FAILED stream must report a statusReason, got {:?}",
                        case.name, read.body
                    )
                });
                assert_eq!(
                    error, expected,
                    "case {:?}: expected statusReason.error {expected}, got {error:?}",
                    case.name
                );
                assert!(
                    STREAM_FAILURE_ERROR_CODE_VALUES.contains(&error.as_str()),
                    "case {:?}: {error:?} is not a published StreamFailureErrorCode ({:?})",
                    case.name,
                    STREAM_FAILURE_ERROR_CODE_VALUES
                );
                let created_at = read.get("creationTime").as_f64().unwrap_or_default();
                assert!(
                    updated_at >= created_at,
                    "case {:?}: statusReason.updatedAt {updated_at} precedes the stream's \
                     creationTime {created_at}",
                    case.name
                );
            }
        }
    }
}

/// RULE: the failed status is the CLOCK's to reach, and the `updatedAt` its
/// `statusReason` carries is the instant it was reached — so a caller reading
/// straight after the create still sees `CREATING` with no reason at all, and
/// every later read reports the same instant.
#[tokio::test]
async fn a_failed_streams_reason_appears_with_the_status_and_is_stable_across_reads() {
    let dsql = Dsql::with_delivery_services(REGION);
    let targets = given_targets(&dsql).await;
    let cluster = dsql.create_settled().await;
    let created = dsql
        .create_stream(
            &cluster,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": targets.definition(&TargetCase {
                    name: "a destination nothing ever created",
                    destination: Destination::Absent,
                    pass: Pass::TrustingThisService,
                    settles_into: STREAM_STATUS_FAILED,
                    reason: Some(STREAM_FAILURE_KINESIS_STREAM_NOT_FOUND),
                }),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateStream must succeed, got {} {:?}",
        created.status, created.body
    );
    let stream = created.str("streamIdentifier");

    let immediately = dsql.get_stream(&cluster, &stream).await;
    assert_eq!(
        immediately.str("status"),
        STREAM_STATUS_CREATING,
        "a read straight after the create must still see CREATING, got {:?}",
        immediately.str("status")
    );
    assert!(
        status_reason(&immediately).is_none(),
        "a stream still CREATING reports no statusReason, got {:?}",
        status_reason(&immediately)
    );

    wait_for_the_clock().await;

    let first = dsql.get_stream(&cluster, &stream).await;
    let (error, updated_at) = status_reason(&first)
        .unwrap_or_else(|| panic!("the settled stream must report a reason, got {:?}", first.body));
    assert_eq!(
        error, STREAM_FAILURE_KINESIS_STREAM_NOT_FOUND,
        "expected statusReason.error {STREAM_FAILURE_KINESIS_STREAM_NOT_FOUND}, got {error:?}"
    );
    let second = dsql.get_stream(&cluster, &stream).await;
    assert_eq!(
        status_reason(&second),
        Some((error.clone(), updated_at)),
        "a second read must report the same statusReason, got {:?}",
        status_reason(&second)
    );
}

/// RULE: a failed stream is an ordinary stream of its cluster — `ListStreams`
/// reports the status it settled into, and the stream can be deleted, after which
/// the reason goes with the failed status it belonged to.
#[tokio::test]
async fn a_failed_stream_is_listed_and_can_be_deleted() {
    let dsql = Dsql::with_delivery_services(REGION);
    let targets = given_targets(&dsql).await;
    let cluster = dsql.create_settled().await;
    let created = dsql
        .create_stream(
            &cluster,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": json!({"kinesis": {
                    "streamArn": targets.stream_arn(Destination::Absent),
                    "roleArn": targets.role_arn(Pass::TrustingThisService),
                }}),
            }),
        )
        .await;
    let stream = created.str("streamIdentifier");
    wait_for_the_clock().await;

    let listed = dsql.list_streams(&cluster, "").await;
    let statuses: Vec<String> = listed
        .get("streams")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        statuses,
        vec![STREAM_STATUS_FAILED.to_string()],
        "ListStreams must report the status the stream settled into, got {:?}",
        listed.body
    );

    let deleted = dsql.delete_stream(&cluster, &stream, "").await;
    assert_eq!(
        deleted.status, 200,
        "a FAILED stream accepts its deletion, got {} {:?}",
        deleted.status, deleted.body
    );
    let deleting = dsql.get_stream(&cluster, &stream).await;
    assert_eq!(
        deleting.str("status"),
        STREAM_STATUS_DELETING,
        "the deleted stream must report DELETING, got {:?}",
        deleting.str("status")
    );
    assert!(
        status_reason(&deleting).is_none(),
        "the reason belongs to the failed status, so a DELETING stream reports none, got {:?}",
        status_reason(&deleting)
    );

    wait_for_the_clock().await;
    let gone = dsql.get_stream(&cluster, &stream).await;
    assert_eq!(
        gone.status, 404,
        "the clock must remove the deleted stream, got {} {:?}",
        gone.status, gone.body
    );
}
