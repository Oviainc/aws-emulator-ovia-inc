// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the data-grant plane — `CreateDataGrant`, `GetDataGrant`,
//! `AcceptDataGrant`, `DeleteDataGrant` — and for `RevokeRevision`, the one operation
//! whose precondition is that a data grant exists.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator, the request deserializer and the serialized body
//! are all exercised. The harness (`emulator`, `harness`, `call`, `create_data_set`,
//! `seconds`) is the one `data_sets.rs` already uses.
//!
//! Four rules are under test.
//!
//! 1. A request-body member whose modelled target carries `@timestampFormat("date-time")`
//!    is read off the wire as an RFC 3339 `date-time` STRING — the only rendering a
//!    generated SDK client sends for such a member — and the instant it names is what the
//!    service stores and answers. The rows span the forms RFC 3339 admits (whole seconds,
//!    fractional seconds, instants far either side of now) plus the absent member and a
//!    string that names no instant at all.
//! 2. A created data grant is `PENDING_RECEIVER_ACCEPTANCE`, its `SenderPrincipal` is the
//!    calling account and it carries no `AcceptedAt`; the single-grant reader reports the
//!    same record, addressed by its id or by its ARN, and a delete is the only thing that
//!    takes it away.
//! 3. A data grant is accepted by the account it was made out to: that call moves it to
//!    `ACCEPTED` and stamps `AcceptedAt`, and the same call from any other account is the
//!    modelled 404 and changes nothing.
//! 4. Revoking a revision withdraws subscribers' access, so it is admitted only while
//!    something shares that revision's data set; otherwise it is the modelled 409. Once a
//!    revision is revoked it admits no further action, so a second revoke and any update
//!    of it are refused too.

use std::sync::Arc;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::ShapeType;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{
    call, create_data_set, emulator, harness, is_modelled_id, seconds, Answer, ACCOUNT, REGION,
};
use crate::resource_tags::percent_encode;
use crate::revision_finalization::import_assets;
use crate::revisions::create_revision;

use dataexchange::ops::types as shapes;

/// Well-formed per the modelled `Id` pattern `^[a-zA-Z0-9]{30,40}$` (32 characters) but
/// never created.
const NEVER_CREATED: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

/// Every value the model's `com.amazonaws.dataexchange#GrantDistributionScope` enum
/// admits. Source: `smithy_query(query="shape", shape="GrantDistributionScope")`,
/// mirrored by `crate::ops::types::GRANT_DISTRIBUTION_SCOPE_VALUES`.
const GRANT_DISTRIBUTION_SCOPES: &[&str] = &["AWS_ORGANIZATION", "NONE"];

/// The `AcceptanceState` the model gives a grant nobody has accepted
/// (`smithy_query(query="shape", shape="DataGrantAcceptanceState")` ->
/// `["PENDING_RECEIVER_ACCEPTANCE", "ACCEPTED"]`).
const PENDING: &str = "PENDING_RECEIVER_ACCEPTANCE";

/// The `AcceptanceState` of an accepted grant, from the same value set.
const ACCEPTED: &str = "ACCEPTED";

/// The whole `Message` of this service's modelled 404 (`crate::state`'s
/// `RESOURCE_NOT_FOUND_MESSAGE`).
const NOT_FOUND: &str = "The requested resource was not found.";

/// The refusal of a revoke of a revision nothing shares.
const NOT_SHARED: &str = "Can only revoke a Revision that is included in a data product or data grant.";

/// The refusal of any action on a revision whose access has already been revoked.
const NO_ACTION: &str = "Cannot take action on a Finalized Revision.";

/// A `RevocationComment` satisfying the modelled `@length(min: 10, max: 512)`.
const REVOCATION_COMMENT: &str = "withdrawn for review";

/// Send `CreateDataGrant`.
async fn create_data_grant(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/v1/data-grants", Some(body)).await
}

/// Read one data grant back, by its id or by its ARN (the member admits both).
async fn get_data_grant(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/data-grants/{}", percent_encode(identifier)),
        None,
    )
    .await
}

/// Accept one data grant, which the model addresses by ARN.
async fn accept_data_grant(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/data-grants/{}/accept", percent_encode(arn)),
        None,
    )
    .await
}

/// Delete one data grant, by its id or by its ARN.
async fn delete_data_grant(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/data-grants/{}", percent_encode(identifier)),
        None,
    )
    .await
}

/// Send `RevokeRevision`.
async fn revoke_revision(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
    comment: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}/revoke"),
        Some(json!({ "RevocationComment": comment })),
    )
    .await
}

/// Send `UpdateRevision`.
async fn update_revision(
    router: &MultiServiceRouter,
    data_set_id: &str,
    revision_id: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "PATCH",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
        Some(body),
    )
    .await
}

/// Read one revision back through `GetRevision`.
async fn get_revision(router: &MultiServiceRouter, data_set_id: &str, revision_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
        None,
    )
    .await
}

/// The refusal message an answer carries, or the empty string when it carries none.
fn message(answer: &Answer) -> String {
    answer
        .member("Message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The instant a member names, having first asserted it is on the wire in the RFC 3339
/// `date-time` form rather than as a JSON number.
fn instant(case: &str, member: &str, value: &Value) -> DateTime {
    assert!(
        !value.is_number(),
        "case {case}: expected {member} as an RFC 3339 date-time string, got the JSON \
         number {value:?}"
    );
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected {member} as a JSON string, got {value:?}"));
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case}: {member}={text:?} is not an RFC 3339 date-time: {error}")
    })
}

/// A data set holding one finalized revision, for the revoke rows: the state a revision
/// has to be in before subscribers can have access to it withdrawn.
async fn finalized_revision(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
) -> (String, String) {
    let data_set_id = create_data_set(router, case, "r", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let revision_id = create_revision(router, &data_set_id, Some("first cut"), None, case)
        .await
        .string("Id");
    import_assets(engine, &data_set_id, &revision_id, 1);
    let finalized = update_revision(
        router,
        &data_set_id,
        &revision_id,
        json!({ "Finalized": true }),
    )
    .await;
    assert_eq!(
        finalized.status, 200,
        "case {case}: expected 200 from UpdateRevision(Finalized=true), got {} body {:?}",
        finalized.status, finalized.body
    );
    (data_set_id, revision_id)
}

/// RULE (schema): the member schema is what decides how the codec reads an instant, so
/// every member of the create's INPUT shape that the model targets at `Timestamp` reports
/// the `date-time` format, and every other member reports none.
#[test]
fn the_request_shape_carries_the_date_time_format() {
    let schema = shapes::CREATE_DATA_GRANT_INPUT_SCHEMA;
    let timestamps: Vec<&str> = schema
        .members()
        .iter()
        .filter(|member| matches!(member.shape_type(), ShapeType::Timestamp))
        .filter_map(|member| member.member_name())
        .collect();
    assert_eq!(
        timestamps,
        vec!["EndsAt"],
        "case CreateDataGrantRequest: expected the model's timestamp members [\"EndsAt\"], \
         got {timestamps:?}"
    );
    for (position, member) in schema.members().iter().enumerate() {
        let name = member.member_name().unwrap_or("<unnamed>");
        let format = member.timestamp_format().map(|trait_| trait_.format());
        if matches!(member.shape_type(), ShapeType::Timestamp) {
            assert!(
                matches!(format, Some(TimestampFormat::DateTime)),
                "case CreateDataGrantRequest.{name}: the model marks this member \
                 @timestampFormat(\"date-time\"), so its schema must carry that format, \
                 got {format:?}"
            );
        } else {
            assert!(
                format.is_none(),
                "case CreateDataGrantRequest.{name}: the model gives this member no \
                 @timestampFormat, got {format:?}"
            );
        }
        assert_eq!(
            member.member_index(),
            Some(position),
            "case CreateDataGrantRequest.{name}: the member must report the index it sits \
             at ({position}), got {:?}",
            member.member_index()
        );
    }
}

/// RULE: a `date-time` body member is read as an RFC 3339 string, whatever instant it
/// names, and the instant it names is the one the service stores and answers.
#[tokio::test]
async fn ends_at_is_read_as_an_rfc3339_string() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "grant-timestamps", "t", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // The rows differ along the FORM of the instant: whole seconds, fractional seconds, a
    // leap day, the far future, an instant in the past, and the one minute of a day's end.
    let rows: &[(&str, &str)] = &[
        ("whole seconds", "2026-11-28T12:00:00Z"),
        ("fractional seconds", "2026-11-28T12:00:00.123Z"),
        ("a leap day", "2028-02-29T23:59:59Z"),
        ("the far future", "2099-12-31T23:59:59Z"),
        ("an instant in the past", "2001-09-09T01:46:40Z"),
        ("midnight", "2030-01-01T00:00:00Z"),
    ];

    for (case, ends_at) in rows {
        let created = create_data_grant(
            &router,
            json!({
                "Name": format!("grant for {case}"),
                "GrantDistributionScope": "NONE",
                "ReceiverPrincipal": "111122223333",
                "SourceDataSetId": data_set_id,
                "EndsAt": ends_at,
            }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateDataGrant for EndsAt={ends_at:?}, got {} \
             body {:?}",
            created.status, created.body
        );
        let answered = instant(case, "EndsAt", created.member("EndsAt"));
        let sent = DateTime::from_str(ends_at, Format::DateTime)
            .unwrap_or_else(|error| panic!("case {case}: the row's own value parses: {error}"));
        assert_eq!(
            answered.as_secs_f64(),
            sent.as_secs_f64(),
            "case {case}: expected the instant {ends_at:?} names, got {:?}",
            created.member("EndsAt")
        );

        // Round trip: the single-grant reader reports the instant that was stored.
        let fetched = get_data_grant(&router, &created.string("Id")).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected 200 from GetDataGrant, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            instant(case, "EndsAt", fetched.member("EndsAt")).as_secs_f64(),
            sent.as_secs_f64(),
            "case {case}: expected GetDataGrant to report the stored instant, got {:?}",
            fetched.member("EndsAt")
        );
    }

    // `EndsAt` is optional: a grant created without one carries no such member at all.
    let open_ended = create_data_grant(
        &router,
        json!({
            "Name": "open-ended grant",
            "GrantDistributionScope": "NONE",
            "ReceiverPrincipal": "111122223333",
            "SourceDataSetId": data_set_id,
        }),
    )
    .await;
    assert_eq!(
        open_ended.status, 201,
        "case no EndsAt: expected 201 from CreateDataGrant, got {} body {:?}",
        open_ended.status, open_ended.body
    );
    assert_eq!(
        open_ended.member("EndsAt"),
        &Value::Null,
        "case no EndsAt: expected no EndsAt member, got {:?}",
        open_ended.member("EndsAt")
    );

    // And a string that names no instant is the modelled 400, which is what that refusal
    // is reserved for.
    for (case, ends_at) in [
        ("not a timestamp at all", "next thursday"),
        ("a date with no time", "2026-11-28"),
        ("a time with no zone", "2026-11-28T12:00:00"),
        ("an impossible month", "2026-13-28T12:00:00Z"),
    ] {
        let refused = create_data_grant(
            &router,
            json!({
                "Name": "malformed timestamp",
                "GrantDistributionScope": "NONE",
                "ReceiverPrincipal": "111122223333",
                "SourceDataSetId": data_set_id,
                "EndsAt": ends_at,
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400 for EndsAt={ends_at:?}, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException for EndsAt={ends_at:?}, got {:?}",
            refused.error
        );
    }
}

/// One row of the create table: how a failure names the case, the `GrantDistributionScope`
/// and `ReceiverPrincipal` the request carries, and the optional `Description`, `EndsAt`
/// and `Tags` it carries.
type CreateRow = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<Value>,
);

/// RULE: a created data grant is pending its receiver's acceptance, names this account as
/// its sender and carries no `AcceptedAt`; the single-grant reader reports the same
/// record, whether it is addressed by id or by ARN.
#[tokio::test]
async fn a_created_grant_is_pending_receiver_acceptance() {
    let router = emulator();

    // The rows span the shape of the request: both values of the modelled
    // `GrantDistributionScope`, with and without the optional `Description`, `EndsAt` and
    // `Tags`, and a receiver that is this account as well as one that is not.
    let rows: Vec<CreateRow> = vec![
        ("scope NONE, bare", "NONE", "111122223333", None, None, None),
        (
            "scope AWS_ORGANIZATION, described",
            "AWS_ORGANIZATION",
            "444455556666",
            Some("the quarterly feed"),
            None,
            None,
        ),
        (
            "with an end and tags",
            "NONE",
            "777788889999",
            Some("ends in a month"),
            Some("2026-12-28T00:00:00Z"),
            Some(json!({ "team": "data", "tier": "gold" })),
        ),
        (
            "granted to this account",
            "NONE",
            ACCOUNT,
            None,
            Some("2031-06-01T08:30:00Z"),
            None,
        ),
    ];

    for (case, scope, receiver, description, ends_at, tags) in rows {
        let data_set = create_data_set(&router, case, "d", "S3_SNAPSHOT", None).await;
        let data_set_id = data_set.string("Id");
        let mut body = json!({
            "Name": format!("grant {case}"),
            "GrantDistributionScope": scope,
            "ReceiverPrincipal": receiver,
            "SourceDataSetId": data_set_id,
        });
        if let Some(description) = description {
            body["Description"] = json!(description);
        }
        if let Some(ends_at) = ends_at {
            body["EndsAt"] = json!(ends_at);
        }
        if let Some(tags) = &tags {
            body["Tags"] = tags.clone();
        }

        let created = create_data_grant(&router, body).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateDataGrant, got {} body {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.member("AcceptanceState"),
            &json!(PENDING),
            "case {case}: expected a created grant to be {PENDING}, got {:?}",
            created.member("AcceptanceState")
        );
        assert_eq!(
            created.member("SenderPrincipal"),
            &json!(ACCOUNT),
            "case {case}: expected the calling account as SenderPrincipal, got {:?}",
            created.member("SenderPrincipal")
        );
        assert_eq!(
            created.member("ReceiverPrincipal"),
            &json!(receiver),
            "case {case}: expected the receiver the request named, got {:?}",
            created.member("ReceiverPrincipal")
        );
        assert_eq!(
            created.member("AcceptedAt"),
            &Value::Null,
            "case {case}: expected no AcceptedAt on an unaccepted grant, got {:?}",
            created.member("AcceptedAt")
        );
        assert_eq!(
            created.member("GrantDistributionScope"),
            &json!(scope),
            "case {case}: expected the scope the request named, got {:?}",
            created.member("GrantDistributionScope")
        );
        assert_eq!(
            created.member("SourceDataSetId"),
            &json!(data_set_id),
            "case {case}: expected the source data set the request named, got {:?}",
            created.member("SourceDataSetId")
        );
        let grant_id = created.string("Id");
        assert!(
            is_modelled_id(&grant_id),
            "case {case}: expected a modelled Id, got {grant_id:?}"
        );
        assert_eq!(
            created.string("Arn"),
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-grants/{grant_id}"),
            "case {case}: expected the grant's own ARN, got {:?}",
            created.member("Arn")
        );
        let created_at = instant(case, "CreatedAt", created.member("CreatedAt"));
        assert_eq!(
            instant(case, "UpdatedAt", created.member("UpdatedAt")).as_secs_f64(),
            created_at.as_secs_f64(),
            "case {case}: expected UpdatedAt to equal CreatedAt on a fresh grant"
        );
        assert_eq!(
            created.member("Tags"),
            &tags.clone().unwrap_or(Value::Null),
            "case {case}: expected the tags the request carried, got {:?}",
            created.member("Tags")
        );

        // Round trip, by id and by ARN: both address the same stored record.
        for identifier in [grant_id.clone(), created.string("Arn")] {
            let fetched = get_data_grant(&router, &identifier).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: expected 200 from GetDataGrant({identifier}), got {} body {:?}",
                fetched.status, fetched.body
            );
            for member in [
                "Name",
                "SenderPrincipal",
                "ReceiverPrincipal",
                "Description",
                "AcceptanceState",
                "AcceptedAt",
                "EndsAt",
                "GrantDistributionScope",
                "DataSetId",
                "SourceDataSetId",
                "Id",
                "Arn",
                "CreatedAt",
                "UpdatedAt",
                "Tags",
            ] {
                assert_eq!(
                    fetched.member(member),
                    created.member(member),
                    "case {case}: expected GetDataGrant({identifier})'s {member} to equal \
                     what the create answered, got {:?} for {:?}",
                    fetched.member(member),
                    created.member(member)
                );
            }
        }
    }

    // An id naming no grant, and a data set id used as one, are the modelled 404.
    for (case, identifier) in [
        ("an id naming no grant", NEVER_CREATED.to_string()),
        (
            "an ARN naming no grant",
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-grants/{NEVER_CREATED}"),
        ),
    ] {
        let answer = get_data_grant(&router, &identifier).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.error
        );
        assert_eq!(
            message(&answer),
            NOT_FOUND,
            "case {case}: expected the service's not-found wording, got {:?}",
            message(&answer)
        );
        assert_eq!(
            answer.member("ResourceType"),
            &json!("DATA_GRANT"),
            "case {case}: expected ResourceType DATA_GRANT, got {:?}",
            answer.member("ResourceType")
        );
    }
}

/// RULE: a data grant is accepted by the account it was made out to — that call moves it
/// to `ACCEPTED` and stamps `AcceptedAt` — and the same call from any other account is the
/// modelled 404 and leaves the grant pending.
#[tokio::test]
async fn only_the_receiver_can_accept_a_grant() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "grant-accept", "a", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // The rows differ along the receiver the grant names: this account (which can accept
    // it) and three other well-formed accounts (which cannot). Each scope is covered, so
    // the identity check is not keyed to one shape of grant.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("the calling account", ACCOUNT, "NONE"),
        ("another account", "111122223333", "NONE"),
        ("an account of all nines", "999999999999", "AWS_ORGANIZATION"),
        ("an account of all zeros but one", "000000000001", "NONE"),
    ];

    for (case, receiver, scope) in rows {
        let created = create_data_grant(
            &router,
            json!({
                "Name": format!("grant for {case}"),
                "GrantDistributionScope": scope,
                "ReceiverPrincipal": receiver,
                "SourceDataSetId": data_set_id,
            }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateDataGrant, got {} body {:?}",
            created.status, created.body
        );
        let grant_id = created.string("Id");
        let arn = created.string("Arn");

        let accepted = accept_data_grant(&router, &arn).await;
        if receiver == ACCOUNT {
            assert_eq!(
                accepted.status, 200,
                "case {case}: expected 200 from AcceptDataGrant, got {} body {:?}",
                accepted.status, accepted.body
            );
            assert_eq!(
                accepted.member("AcceptanceState"),
                &json!(ACCEPTED),
                "case {case}: expected the grant to be {ACCEPTED}, got {:?}",
                accepted.member("AcceptanceState")
            );
            let accepted_at = instant(case, "AcceptedAt", accepted.member("AcceptedAt"));
            assert!(
                accepted_at.as_secs_f64()
                    >= seconds(created.member("CreatedAt"), case),
                "case {case}: expected AcceptedAt at or after CreatedAt, got {} for {}",
                accepted_at.as_secs_f64(),
                seconds(created.member("CreatedAt"), case)
            );

            // Round trip: the reader reports the accepted record.
            let fetched = get_data_grant(&router, &grant_id).await;
            assert_eq!(
                fetched.member("AcceptanceState"),
                &json!(ACCEPTED),
                "case {case}: expected GetDataGrant to report {ACCEPTED}, got {:?}",
                fetched.member("AcceptanceState")
            );
            assert_eq!(
                fetched.member("AcceptedAt"),
                accepted.member("AcceptedAt"),
                "case {case}: expected GetDataGrant to report the stored AcceptedAt, got \
                 {:?} for {:?}",
                fetched.member("AcceptedAt"),
                accepted.member("AcceptedAt")
            );

            // A grant is accepted once: a repeat of the same call reports the instant the
            // first accept recorded rather than a later one.
            let again = accept_data_grant(&router, &arn).await;
            assert_eq!(
                again.member("AcceptedAt"),
                accepted.member("AcceptedAt"),
                "case {case}: expected a repeated accept to keep the first AcceptedAt, got \
                 {:?} for {:?}",
                again.member("AcceptedAt"),
                accepted.member("AcceptedAt")
            );
        } else {
            assert_eq!(
                accepted.status, 404,
                "case {case}: expected 404 from AcceptDataGrant by a non-receiver, got {} \
                 body {:?}",
                accepted.status, accepted.body
            );
            assert_eq!(
                accepted.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException, got {:?}",
                accepted.error
            );
            assert_eq!(
                message(&accepted),
                NOT_FOUND,
                "case {case}: expected the service's not-found wording, got {:?}",
                message(&accepted)
            );

            // And the grant is untouched.
            let fetched = get_data_grant(&router, &grant_id).await;
            assert_eq!(
                fetched.member("AcceptanceState"),
                &json!(PENDING),
                "case {case}: expected the grant to stay {PENDING}, got {:?}",
                fetched.member("AcceptanceState")
            );
            assert_eq!(
                fetched.member("AcceptedAt"),
                &Value::Null,
                "case {case}: expected no AcceptedAt after a refused accept, got {:?}",
                fetched.member("AcceptedAt")
            );
        }
    }
}

/// RULE: a delete takes the grant away — a later read of it is the modelled 404, and so is
/// a second delete — and it takes away only the grant it names.
#[tokio::test]
async fn deleting_a_grant_removes_it() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "grant-delete", "x", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // The rows differ along how the delete addresses the grant (by id, by ARN) and along
    // the grant's acceptance state when it is deleted.
    for (case, by_arn, accept_first) in [
        ("by id, pending", false, false),
        ("by ARN, pending", true, false),
        ("by id, accepted", false, true),
    ] {
        let keeper = create_data_grant(
            &router,
            json!({
                "Name": format!("kept while deleting {case}"),
                "GrantDistributionScope": "NONE",
                "ReceiverPrincipal": "111122223333",
                "SourceDataSetId": data_set_id,
            }),
        )
        .await;
        let keeper_id = keeper.string("Id");

        let created = create_data_grant(
            &router,
            json!({
                "Name": format!("grant {case}"),
                "GrantDistributionScope": "NONE",
                "ReceiverPrincipal": ACCOUNT,
                "SourceDataSetId": data_set_id,
            }),
        )
        .await;
        let grant_id = created.string("Id");
        let arn = created.string("Arn");
        if accept_first {
            let accepted = accept_data_grant(&router, &arn).await;
            assert_eq!(
                accepted.status, 200,
                "case {case}: expected 200 from AcceptDataGrant, got {} body {:?}",
                accepted.status, accepted.body
            );
        }
        let identifier = if by_arn { arn.clone() } else { grant_id.clone() };

        let deleted = delete_data_grant(&router, &identifier).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected 204 from DeleteDataGrant, got {} body {:?}",
            deleted.status, deleted.body
        );

        let gone = get_data_grant(&router, &grant_id).await;
        assert_eq!(
            gone.status, 404,
            "case {case}: expected 404 from GetDataGrant after the delete, got {} body {:?}",
            gone.status, gone.body
        );
        let again = delete_data_grant(&router, &identifier).await;
        assert_eq!(
            again.status, 404,
            "case {case}: expected 404 from a second DeleteDataGrant, got {} body {:?}",
            again.status, again.body
        );

        // The other grant of the same data set is still there.
        let kept = get_data_grant(&router, &keeper_id).await;
        assert_eq!(
            kept.status, 200,
            "case {case}: expected the other grant to survive, got {} body {:?}",
            kept.status, kept.body
        );
    }
}

/// RULE: revoking a revision withdraws subscribers' access, so it is admitted only while
/// a data grant shares that revision's data set; otherwise it is the modelled 409 and the
/// revision stays unrevoked.
#[tokio::test]
async fn revoking_a_revision_nothing_shares_is_refused() {
    let (router, engine) = harness();

    // The rows differ along WHAT ELSE is in the store, so the refusal is about this
    // revision's own data set and not about whether any grant exists at all: no grant, a
    // grant on another data set, and a grant that was deleted again.
    for case in [
        "no grant at all",
        "a grant on another data set",
        "a grant that was deleted",
    ] {
        let (data_set_id, revision_id) = finalized_revision(&router, &engine, case).await;
        if case != "no grant at all" {
            let other = create_data_set(&router, case, "o", "S3_SNAPSHOT", None)
                .await
                .string("Id");
            let grant = create_data_grant(
                &router,
                json!({
                    "Name": format!("grant {case}"),
                    "GrantDistributionScope": "NONE",
                    "ReceiverPrincipal": "111122223333",
                    "SourceDataSetId": other,
                }),
            )
            .await;
            assert_eq!(
                grant.status, 201,
                "case {case}: expected 201 from CreateDataGrant, got {} body {:?}",
                grant.status, grant.body
            );
            if case == "a grant that was deleted" {
                let deleted = delete_data_grant(&router, &grant.string("Id")).await;
                assert_eq!(
                    deleted.status, 204,
                    "case {case}: expected 204 from DeleteDataGrant, got {} body {:?}",
                    deleted.status, deleted.body
                );
            }
        }

        let refused = revoke_revision(&router, &data_set_id, &revision_id, REVOCATION_COMMENT).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected 409 from RevokeRevision, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ConflictException"),
            "case {case}: expected ConflictException, got {:?}",
            refused.error
        );
        assert_eq!(
            message(&refused),
            NOT_SHARED,
            "case {case}: expected the service's wording for an unshared revision, got {:?}",
            message(&refused)
        );
        assert_eq!(
            refused.member("ResourceType"),
            &json!("REVISION"),
            "case {case}: expected ResourceType REVISION, got {:?}",
            refused.member("ResourceType")
        );

        // Nothing moved on the revision.
        let fetched = get_revision(&router, &data_set_id, &revision_id).await;
        assert_eq!(
            fetched.member("Revoked"),
            &json!(false),
            "case {case}: expected Revoked to stay false, got {:?}",
            fetched.member("Revoked")
        );
        assert_eq!(
            fetched.member("RevokedAt"),
            &Value::Null,
            "case {case}: expected no RevokedAt, got {:?}",
            fetched.member("RevokedAt")
        );
        assert_eq!(
            fetched.member("RevocationComment"),
            &Value::Null,
            "case {case}: expected no RevocationComment, got {:?}",
            fetched.member("RevocationComment")
        );
    }

    // An id naming no data set, or no revision of it, is the modelled 404 — the state
    // precondition never speaks for a revision that does not exist.
    let (data_set_id, revision_id) = finalized_revision(&router, &engine, "absent").await;
    for (case, data_set, revision, resource_type) in [
        (
            "unknown data set",
            NEVER_CREATED.to_string(),
            revision_id.clone(),
            "DATA_SET",
        ),
        (
            "unknown revision of a known data set",
            data_set_id.clone(),
            NEVER_CREATED.to_string(),
            "REVISION",
        ),
    ] {
        let answer = revoke_revision(&router, &data_set, &revision, REVOCATION_COMMENT).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member("ResourceType"),
            &json!(resource_type),
            "case {case}: expected ResourceType {resource_type}, got {:?}",
            answer.member("ResourceType")
        );
    }
}

/// RULE: once a data grant shares the revision's data set the revoke succeeds, and the
/// revoked revision then admits no further action: a second revoke and any update of it
/// are refused with the modelled 409 and change nothing.
#[tokio::test]
async fn a_revoked_revision_is_read_only() {
    let (router, engine) = harness();

    // The rows differ along the comment the revoke carries (the modelled
    // `@length(min: 10, max: 512)` at its minimum, in the middle and at its maximum) and
    // along whether the grant naming the data set has been accepted.
    let rows: Vec<(&str, String, bool)> = vec![
        ("the shortest admitted comment", "0123456789".to_string(), false),
        ("a plain comment", REVOCATION_COMMENT.to_string(), false),
        ("the longest admitted comment", "r".repeat(512), true),
        (
            "a non-ascii comment",
            "révoqué — 取り消し ✅ for review".to_string(),
            false,
        ),
    ];

    for (case, comment, accept_grant) in rows {
        let (data_set_id, revision_id) = finalized_revision(&router, &engine, case).await;
        let grant = create_data_grant(
            &router,
            json!({
                "Name": format!("grant {case}"),
                "GrantDistributionScope": "NONE",
                "ReceiverPrincipal": ACCOUNT,
                "SourceDataSetId": data_set_id,
            }),
        )
        .await;
        assert_eq!(
            grant.status, 201,
            "case {case}: expected 201 from CreateDataGrant, got {} body {:?}",
            grant.status, grant.body
        );
        if accept_grant {
            let accepted = accept_data_grant(&router, &grant.string("Arn")).await;
            assert_eq!(
                accepted.status, 200,
                "case {case}: expected 200 from AcceptDataGrant, got {} body {:?}",
                accepted.status, accepted.body
            );
        }

        let revoked = revoke_revision(&router, &data_set_id, &revision_id, &comment).await;
        assert_eq!(
            revoked.status, 200,
            "case {case}: expected 200 from RevokeRevision, got {} body {:?}",
            revoked.status, revoked.body
        );
        assert_eq!(
            revoked.member("Revoked"),
            &json!(true),
            "case {case}: expected Revoked=true, got {:?}",
            revoked.member("Revoked")
        );
        assert_eq!(
            revoked.member("RevocationComment"),
            &json!(comment),
            "case {case}: expected the comment the request carried, got {:?}",
            revoked.member("RevocationComment")
        );
        let revoked_at = instant(case, "RevokedAt", revoked.member("RevokedAt"));
        assert!(
            revoked_at.as_secs_f64() >= seconds(revoked.member("CreatedAt"), case),
            "case {case}: expected RevokedAt at or after CreatedAt, got {} for {}",
            revoked_at.as_secs_f64(),
            seconds(revoked.member("CreatedAt"), case)
        );
        assert_eq!(
            revoked.member("Finalized"),
            &json!(true),
            "case {case}: expected the revision to stay finalized, got {:?}",
            revoked.member("Finalized")
        );

        // Round trip: the single-revision reader reports what the revoke persisted.
        let fetched = get_revision(&router, &data_set_id, &revision_id).await;
        for member in ["Revoked", "RevocationComment", "RevokedAt", "UpdatedAt"] {
            assert_eq!(
                fetched.member(member),
                revoked.member(member),
                "case {case}: expected GetRevision's {member} to equal what the revoke \
                 answered, got {:?} for {:?}",
                fetched.member(member),
                revoked.member(member)
            );
        }

        // A second revoke is refused, and the first one's record is untouched.
        let twice = revoke_revision(&router, &data_set_id, &revision_id, "another reason").await;
        assert_eq!(
            twice.status, 409,
            "case {case}: expected 409 from a second RevokeRevision, got {} body {:?}",
            twice.status, twice.body
        );
        assert_eq!(
            message(&twice),
            NO_ACTION,
            "case {case}: expected the service's wording for a revoked revision, got {:?}",
            message(&twice)
        );

        // And so is every update of it.
        for (how, body) in [
            ("a comment", json!({ "Comment": "second cut" })),
            ("un-finalizing", json!({ "Finalized": false })),
            ("re-finalizing", json!({ "Finalized": true })),
        ] {
            let label = format!("{case}, {how}");
            let answer = update_revision(&router, &data_set_id, &revision_id, body).await;
            assert_eq!(
                answer.status, 409,
                "case {label}: expected 409 from UpdateRevision, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ConflictException"),
                "case {label}: expected ConflictException, got {:?}",
                answer.error
            );
            assert_eq!(
                message(&answer),
                NO_ACTION,
                "case {label}: expected the service's wording for a revoked revision, got \
                 {:?}",
                message(&answer)
            );
        }
        let after = get_revision(&router, &data_set_id, &revision_id).await;
        for member in [
            "Comment",
            "Finalized",
            "Revoked",
            "RevocationComment",
            "RevokedAt",
            "UpdatedAt",
        ] {
            assert_eq!(
                after.member(member),
                fetched.member(member),
                "case {case}: expected {member} to be unchanged by the refused calls, got \
                 {:?} for {:?}",
                after.member(member),
                fetched.member(member)
            );
        }
    }
}

/// RULE: a grant's `GrantDistributionScope` is one of the modelled values, and a value
/// outside that set — like any member outside a modelled constraint — is the modelled 400
/// rather than a stored grant.
#[tokio::test]
async fn a_grant_outside_the_modelled_constraints_is_refused() {
    let (router, engine) = harness();
    let data_set_id = create_data_set(&router, "grant-constraints", "c", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    // Each row breaks ONE modelled constraint of the create's input.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a scope outside the enum",
            json!({
                "Name": "bad scope",
                "GrantDistributionScope": "EVERYONE",
                "ReceiverPrincipal": "111122223333",
                "SourceDataSetId": data_set_id,
            }),
        ),
        (
            "a scope in the wrong case",
            json!({
                "Name": "bad scope case",
                "GrantDistributionScope": "none",
                "ReceiverPrincipal": "111122223333",
                "SourceDataSetId": data_set_id,
            }),
        ),
        (
            "a receiver that is not 12 digits",
            json!({
                "Name": "bad receiver",
                "GrantDistributionScope": "NONE",
                "ReceiverPrincipal": "11112222333",
                "SourceDataSetId": data_set_id,
            }),
        ),
        (
            "an empty name",
            json!({
                "Name": "",
                "GrantDistributionScope": "NONE",
                "ReceiverPrincipal": "111122223333",
                "SourceDataSetId": data_set_id,
            }),
        ),
        (
            "a name one past the modelled maximum",
            json!({
                "Name": "n".repeat(257),
                "GrantDistributionScope": "NONE",
                "ReceiverPrincipal": "111122223333",
                "SourceDataSetId": data_set_id,
            }),
        ),
        (
            "a source data set id outside the modelled pattern",
            json!({
                "Name": "bad source",
                "GrantDistributionScope": "NONE",
                "ReceiverPrincipal": "111122223333",
                "SourceDataSetId": "not-an-id",
            }),
        ),
    ];

    for (case, body) in rows {
        let refused = create_data_grant(&router, body).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400 from CreateDataGrant, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {:?}",
            refused.error
        );
    }

    // A well-formed source data set id that addresses no data set is the modelled 404:
    // there is no data set to share.
    let absent = create_data_grant(
        &router,
        json!({
            "Name": "grant of an absent data set",
            "GrantDistributionScope": "NONE",
            "ReceiverPrincipal": "111122223333",
            "SourceDataSetId": NEVER_CREATED,
        }),
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case an absent source data set: expected 404, got {} body {:?}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.member("ResourceType"),
        &json!("DATA_SET"),
        "case an absent source data set: expected ResourceType DATA_SET, got {:?}",
        absent.member("ResourceType")
    );

    // Nothing was stored by any refused create: the store holds no grant at all.
    let state = dataexchange::state::service_state(&*engine).expect("the dataexchange store");
    let stored = state.lock().data_grants.len();
    assert_eq!(
        stored, 0,
        "case refused creates: expected no stored grant, got {stored}"
    );

    // Every value the enum admits IS accepted, so the refusals above are about the value
    // set and not about the member.
    for scope in GRANT_DISTRIBUTION_SCOPES {
        let created = create_data_grant(
            &router,
            json!({
                "Name": format!("grant with scope {scope}"),
                "GrantDistributionScope": scope,
                "ReceiverPrincipal": "111122223333",
                "SourceDataSetId": data_set_id,
            }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case scope {scope}: expected 201 from CreateDataGrant, got {} body {:?}",
            created.status, created.body
        );
    }
}
