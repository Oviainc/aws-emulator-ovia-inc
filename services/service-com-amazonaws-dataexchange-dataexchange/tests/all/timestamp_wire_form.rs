// SPDX-License-Identifier: CC0-1.0

//! The wire form of every instant this service answers.
//!
//! RULE: every timestamp member this service models targets
//! `com.amazonaws.dataexchange#Timestamp`, and that shape carries
//! `@timestampFormat("date-time")` (`smithy_query(query="shape", shape="Timestamp")`), so
//! in a restJson1 body such a member is an RFC 3339 `date-time` STRING —
//! `YYYY-MM-DDThh:mm:ss(.sss)Z` — and never a JSON number. That holds for the member
//! wherever it sits: at the top level of a response, inside a list entry, and nested under
//! a job's `Details`.
//!
//! Two tables span what the rule generalises over.
//!
//! 1. The SCHEMA table names every shape of this service that carries a timestamp member,
//!    with the member list the model gives it, and checks each such member reports the
//!    `date-time` format while every other member reports none. It covers the shapes no
//!    implemented operation answers yet (the data-grant shapes, the asset readers) and the
//!    notification-detail shapes on the request side, so the rule is pinned for the whole
//!    surface rather than for the operations under test below.
//! 2. The WIRE tables drive the implemented operations end to end through
//!    `MultiServiceRouter::dispatch` and read the serialized body: the data-set and
//!    revision lifecycles over several asset types, a job of each `Type` that announces an
//!    upload URL and its expiry, an imported asset's list entry, and the event-action
//!    lifecycle. Each row asserts the member is a string naming a readable instant, and
//!    that a later read reports the same rendering of the same instant.

use std::sync::Arc;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{call, create_data_set, emulator, harness, Answer};
use crate::job_lifecycle::{get_job, settle_past_the_execution_span, start_job};
use crate::jobs::create_job;
use crate::signed_url_uploads::{base64_md5, put_upload};

use dataexchange::ops::types as shapes;

/// The page size every list under test asks for: the published maximum of
/// `com.amazonaws.dataexchange#MaxResults` (`@range(min: 1, max: 200)`), so one page holds
/// everything these tests create.
const PAGE: i64 = 200;

/// One row of the schema table: how a failure names the shape, the shape's schema, and the
/// members the model targets at `Timestamp`, in the order the model lists them.
type ShapeRow = (&'static str, &'static Schema, &'static [&'static str]);

/// Every shape of this service carrying at least one `Timestamp` member, plus four control
/// rows carrying none.
///
/// Source: `smithy_query(query="shape", shape="<Shape>")` for each shape below; `Timestamp`
/// is the only timestamp shape this service models, and `CreatedAt`, `UpdatedAt`,
/// `RevokedAt`, `AcceptedAt`, `EndsAt`, `DataUpdatedAt`, `DeprecationAt`, `SchemaChangeAt`
/// and the `…ExpiresAt` members are the members that target it.
fn shape_rows() -> Vec<ShapeRow> {
    const CREATED_UPDATED: &[&str] = &["CreatedAt", "UpdatedAt"];
    const REVISION: &[&str] = &["CreatedAt", "UpdatedAt", "RevokedAt"];
    const GRANT: &[&str] = &["AcceptedAt", "EndsAt", "CreatedAt", "UpdatedAt"];
    const NONE: &[&str] = &[];

    vec![
        // Data sets: the three readers/writers and the list projection.
        (
            "CreateDataSetResponse",
            shapes::CREATE_DATA_SET_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        (
            "GetDataSetResponse",
            shapes::GET_DATA_SET_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        (
            "UpdateDataSetResponse",
            shapes::UPDATE_DATA_SET_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        (
            "DataSetEntry",
            shapes::DATA_SET_ENTRY_SCHEMA,
            CREATED_UPDATED,
        ),
        // Revisions, which also carry `RevokedAt`.
        (
            "CreateRevisionResponse",
            shapes::CREATE_REVISION_OUTPUT_SCHEMA,
            REVISION,
        ),
        (
            "GetRevisionResponse",
            shapes::GET_REVISION_OUTPUT_SCHEMA,
            REVISION,
        ),
        (
            "UpdateRevisionResponse",
            shapes::UPDATE_REVISION_OUTPUT_SCHEMA,
            REVISION,
        ),
        (
            "RevokeRevisionResponse",
            shapes::REVOKE_REVISION_OUTPUT_SCHEMA,
            REVISION,
        ),
        ("RevisionEntry", shapes::REVISION_ENTRY_SCHEMA, REVISION),
        // Jobs.
        (
            "CreateJobResponse",
            shapes::CREATE_JOB_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        (
            "GetJobResponse",
            shapes::GET_JOB_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        ("JobEntry", shapes::JOB_ENTRY_SCHEMA, CREATED_UPDATED),
        // Assets.
        ("AssetEntry", shapes::ASSET_ENTRY_SCHEMA, CREATED_UPDATED),
        (
            "GetAssetResponse",
            shapes::GET_ASSET_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        (
            "UpdateAssetResponse",
            shapes::UPDATE_ASSET_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        // Event actions.
        (
            "CreateEventActionResponse",
            shapes::CREATE_EVENT_ACTION_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        (
            "GetEventActionResponse",
            shapes::GET_EVENT_ACTION_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        (
            "UpdateEventActionResponse",
            shapes::UPDATE_EVENT_ACTION_OUTPUT_SCHEMA,
            CREATED_UPDATED,
        ),
        (
            "EventActionEntry",
            shapes::EVENT_ACTION_ENTRY_SCHEMA,
            CREATED_UPDATED,
        ),
        // The nested job details and asset details that announce a URL and its expiry.
        (
            "ExportAssetToSignedUrlResponseDetails",
            shapes::EXPORT_ASSET_TO_SIGNED_URL_RESPONSE_DETAILS_SCHEMA,
            &["SignedUrlExpiresAt"],
        ),
        (
            "ImportAssetFromSignedUrlResponseDetails",
            shapes::IMPORT_ASSET_FROM_SIGNED_URL_RESPONSE_DETAILS_SCHEMA,
            &["SignedUrlExpiresAt"],
        ),
        (
            "ImportAssetFromApiGatewayApiResponseDetails",
            shapes::IMPORT_ASSET_FROM_API_GATEWAY_API_RESPONSE_DETAILS_SCHEMA,
            &["ApiSpecificationUploadUrlExpiresAt"],
        ),
        (
            "ApiGatewayApiAsset",
            shapes::API_GATEWAY_API_ASSET_SCHEMA,
            &["ApiSpecificationDownloadUrlExpiresAt"],
        ),
        // Data grants, whose operations are still scaffolds: the rule is about the member,
        // not about which operation answers it.
        (
            "CreateDataGrantResponse",
            shapes::CREATE_DATA_GRANT_OUTPUT_SCHEMA,
            GRANT,
        ),
        (
            "GetDataGrantResponse",
            shapes::GET_DATA_GRANT_OUTPUT_SCHEMA,
            GRANT,
        ),
        (
            "AcceptDataGrantResponse",
            shapes::ACCEPT_DATA_GRANT_OUTPUT_SCHEMA,
            GRANT,
        ),
        (
            "GetReceivedDataGrantResponse",
            shapes::GET_RECEIVED_DATA_GRANT_OUTPUT_SCHEMA,
            GRANT,
        ),
        (
            "DataGrantSummaryEntry",
            shapes::DATA_GRANT_SUMMARY_ENTRY_SCHEMA,
            GRANT,
        ),
        (
            "ReceivedDataGrantSummariesEntry",
            shapes::RECEIVED_DATA_GRANT_SUMMARIES_ENTRY_SCHEMA,
            GRANT,
        ),
        // The notification details, read off a REQUEST body: the same member format decides
        // how the codec READS an instant.
        (
            "DataUpdateRequestDetails",
            shapes::DATA_UPDATE_REQUEST_DETAILS_SCHEMA,
            &["DataUpdatedAt"],
        ),
        (
            "DeprecationRequestDetails",
            shapes::DEPRECATION_REQUEST_DETAILS_SCHEMA,
            &["DeprecationAt"],
        ),
        (
            "SchemaChangeRequestDetails",
            shapes::SCHEMA_CHANGE_REQUEST_DETAILS_SCHEMA,
            &["SchemaChangeAt"],
        ),
        // Control rows: shapes the model gives no timestamp member at all, so the
        // assertion is about the trait and not about every member schema.
        ("OriginDetails", shapes::ORIGIN_DETAILS_SCHEMA, NONE),
        ("ResponseDetails", shapes::RESPONSE_DETAILS_SCHEMA, NONE),
        ("JobError", shapes::JOB_ERROR_SCHEMA, NONE),
        ("AssetDetails", shapes::ASSET_DETAILS_SCHEMA, NONE),
    ]
}

/// RULE on the schema, which is what decides the rendering: every member the model targets
/// at `Timestamp` is a timestamp member reporting the `date-time` format, every other
/// member reports no format, and each member sits at the index the shape's schema names it
/// at.
#[test]
fn every_timestamp_member_of_every_shape_reports_the_date_time_format() {
    for (shape, schema, expected) in shape_rows() {
        let timestamps: Vec<&str> = schema
            .members()
            .iter()
            .filter(|member| matches!(member.shape_type(), ShapeType::Timestamp))
            .filter_map(|member| member.member_name())
            .collect();
        assert_eq!(
            timestamps, expected,
            "case {shape}: expected the model's timestamp members {expected:?}, got \
             {timestamps:?}"
        );

        for (position, member) in schema.members().iter().enumerate() {
            let name = member.member_name().unwrap_or("<unnamed>");
            let format = member.timestamp_format().map(|trait_| trait_.format());
            if matches!(member.shape_type(), ShapeType::Timestamp) {
                assert!(
                    matches!(format, Some(TimestampFormat::DateTime)),
                    "case {shape}.{name}: the model marks this member \
                     @timestampFormat(\"date-time\"), so its schema must carry that \
                     format, got {format:?}"
                );
            } else {
                assert!(
                    format.is_none(),
                    "case {shape}.{name}: the model gives this member no \
                     @timestampFormat, got {format:?}"
                );
            }
            assert_eq!(
                member.member_index(),
                Some(position),
                "case {shape}.{name}: the member must report the index it sits at \
                 ({position}), got {:?}",
                member.member_index()
            );
        }
    }
}

/// The instant `value` names, having first asserted it is on the wire in the RFC 3339
/// `date-time` form and not as a JSON number.
fn date_time(case: &str, member: &str, value: &Value) -> DateTime {
    assert!(
        !value.is_number(),
        "case {case}: expected {member} as an RFC 3339 date-time string, got the JSON \
         number {value:?}"
    );
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case}: expected {member} as a JSON string, got {value:?}")
    });
    // The form restJson1 writes for a `date-time` member: a date, the `T` separator, a
    // time of day and the `Z` zone, with an optional fractional second.
    assert!(
        text.len() >= "1970-01-01T00:00:00Z".len()
            && text.as_bytes()[10] == b'T'
            && text.ends_with('Z'),
        "case {case}: expected {member} in the YYYY-MM-DDThh:mm:ss(.sss)Z form, got \
         {text:?}"
    );
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case}: {member}={text:?} is not an RFC 3339 date-time: {error}")
    });
    let rendered = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case}: {member}={text:?} renders back: {error}"));
    assert_eq!(
        rendered, text,
        "case {case}: expected {member} to be the canonical rendering of the instant it \
         names, got {text:?} for {rendered:?}"
    );
    parsed
}

/// Assert every `member` of `answer` is a `date-time` string, and hand back the rendering
/// of each so a later read can be compared against it.
fn date_time_members(case: &str, answer: &Answer, members: &[&str]) -> Vec<String> {
    members
        .iter()
        .map(|member| {
            date_time(case, member, answer.member(member));
            answer.string(member)
        })
        .collect()
}

/// RULE applied to the data-set lifecycle: `CreateDataSet`, `GetDataSet`, `UpdateDataSet`
/// and each `ListDataSets` entry answer `CreatedAt`/`UpdatedAt` as `date-time` strings,
/// and a read reports the same rendering the create answered.
///
/// Rows differ along `AssetType` — the member is the same whatever kind of data set carries
/// it — and along whether the data set carries tags.
#[tokio::test]
async fn the_data_set_lifecycle_answers_date_time_strings() {
    let router = emulator();
    // Three of the five values of the model's `AssetType` enum (`smithy_query(query="shape",
    // shape="AssetType")`), paired with no tags and with tags.
    let rows: [(&str, Option<Value>); 3] = [
        ("S3_SNAPSHOT", None),
        ("API_GATEWAY_API", Some(json!({ "team": "data" }))),
        ("REDSHIFT_DATA_SHARE", Some(json!({ "tier": "gold" }))),
    ];

    for (asset_type, tags) in rows {
        let case = format!("asset_type={asset_type} tags={tags:?}");
        let created = create_data_set(
            &router,
            &format!("instants-{asset_type}"),
            "instants on the wire",
            asset_type,
            tags,
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateDataSet, got {} body {:?}",
            created.status, created.body
        );
        let id = created.string("Id");
        let answered = date_time_members(&case, &created, &["CreatedAt", "UpdatedAt"]);

        let fetched = call(&router, "GET", &format!("/v1/data-sets/{id}"), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected 200 from GetDataSet, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            date_time_members(&case, &fetched, &["CreatedAt", "UpdatedAt"]),
            answered,
            "case {case}: expected GetDataSet to report the instants the create answered"
        );

        let updated = call(
            &router,
            "PATCH",
            &format!("/v1/data-sets/{id}"),
            Some(json!({ "Name": format!("renamed-{asset_type}") })),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateDataSet, got {} body {:?}",
            updated.status, updated.body
        );
        let after = date_time_members(&case, &updated, &["CreatedAt", "UpdatedAt"]);
        assert_eq!(
            after[0], answered[0],
            "case {case}: expected CreatedAt to stay where the create left it"
        );
        assert!(
            date_time(&case, "UpdatedAt", updated.member("UpdatedAt")).as_secs_f64()
                >= date_time(&case, "CreatedAt", updated.member("CreatedAt")).as_secs_f64(),
            "case {case}: expected UpdatedAt at or after CreatedAt, got {:?} vs {:?}",
            updated.member("UpdatedAt"),
            updated.member("CreatedAt")
        );

        let listed = call(
            &router,
            "GET",
            &format!("/v1/data-sets?maxResults={PAGE}"),
            None,
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from ListDataSets, got {} body {:?}",
            listed.status, listed.body
        );
        let entry = entry_with_id(&case, listed.member("DataSets"), &id);
        for member in ["CreatedAt", "UpdatedAt"] {
            date_time(&case, member, &entry[member]);
        }
        assert_eq!(
            entry["CreatedAt"].as_str(),
            Some(answered[0].as_str()),
            "case {case}: expected the list entry to carry the stored CreatedAt, got {:?}",
            entry["CreatedAt"]
        );
    }
}

/// The entry of `list` whose `Id` is `id`, cloned so the borrow of the answer ends here.
fn entry_with_id(case: &str, list: &Value, id: &str) -> Value {
    list.as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a list, got {list:?}"))
        .iter()
        .find(|entry| entry["Id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("case {case}: expected an entry for {id} in {list:?}"))
        .clone()
}

/// RULE applied to the revision lifecycle: `CreateRevision`, `GetRevision`,
/// `UpdateRevision` and each `ListDataSetRevisions` entry answer `CreatedAt`/`UpdatedAt` as
/// `date-time` strings, and a revision nothing revoked carries no `RevokedAt` member at all
/// rather than a number.
///
/// Rows differ along the comment the revision carries — none, short, and many kilobytes —
/// because the instants are the same members whatever else the record holds.
#[tokio::test]
async fn the_revision_lifecycle_answers_date_time_strings() {
    let router = emulator();
    let data_set_id = create_data_set(&router, "revision-instants", "d", "S3_SNAPSHOT", None)
        .await
        .string("Id");

    let rows: [(&str, Option<String>); 3] = [
        ("no comment", None),
        ("a short comment", Some("first cut".to_string())),
        ("a long comment", Some("c".repeat(8_192))),
    ];

    for (case, comment) in rows {
        let body = match &comment {
            Some(comment) => json!({ "Comment": comment }),
            None => json!({}),
        };
        let created = call(
            &router,
            "POST",
            &format!("/v1/data-sets/{data_set_id}/revisions"),
            Some(body),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateRevision, got {} body {:?}",
            created.status, created.body
        );
        let revision_id = created.string("Id");
        let answered = date_time_members(case, &created, &["CreatedAt", "UpdatedAt"]);
        assert!(
            created.body.get("RevokedAt").is_none(),
            "case {case}: expected no RevokedAt on a revision nothing revoked, got {:?}",
            created.member("RevokedAt")
        );

        let path = format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}");
        let fetched = call(&router, "GET", &path, None).await;
        assert_eq!(
            date_time_members(case, &fetched, &["CreatedAt", "UpdatedAt"]),
            answered,
            "case {case}: expected GetRevision to report the instants the create answered"
        );

        let finalized = call(
            &router,
            "PATCH",
            &path,
            // A comment change rather than a finalization: finalizing a revision that holds
            // no asset is refused, and what is under test here is the instants an accepted
            // update answers.
            Some(json!({ "Comment": format!("updated: {case}") })),
        )
        .await;
        assert_eq!(
            finalized.status, 200,
            "case {case}: expected 200 from UpdateRevision, got {} body {:?}",
            finalized.status, finalized.body
        );
        let after = date_time_members(case, &finalized, &["CreatedAt", "UpdatedAt"]);
        assert_eq!(
            after[0], answered[0],
            "case {case}: expected CreatedAt to stay where the create left it"
        );

        let listed = call(
            &router,
            "GET",
            &format!("/v1/data-sets/{data_set_id}/revisions?maxResults={PAGE}"),
            None,
        )
        .await;
        let entry = entry_with_id(case, listed.member("Revisions"), &revision_id);
        for member in ["CreatedAt", "UpdatedAt"] {
            date_time(case, member, &entry[member]);
        }
    }
}

/// RULE applied to jobs, including the instant nested under `Details`: a job's
/// `CreatedAt`/`UpdatedAt` and the expiry of the upload URL its details announce are all
/// `date-time` strings, from `CreateJob` and from `GetJob` alike, and the expiry names an
/// instant after the creation one.
///
/// Rows are the two `Type`s whose response details announce a URL and its expiry
/// (`smithy_query(query="shape",
/// shape="ImportAssetFromSignedUrlResponseDetails,ImportAssetFromApiGatewayApiResponseDetails")`),
/// each against the `AssetType` of data set it imports into.
#[tokio::test]
async fn job_answers_carry_date_time_strings_at_every_depth() {
    let router = emulator();
    let content = b"id,name\n1,ada\n".to_vec();
    let digest = base64_md5(&content);

    for (case, asset_type, job_type, member, expiry_member, detail) in [
        (
            "an upload of asset content",
            "S3_SNAPSHOT",
            "IMPORT_ASSET_FROM_SIGNED_URL",
            "ImportAssetFromSignedUrl",
            "SignedUrlExpiresAt",
            json!({ "AssetName": "orders.csv", "Md5Hash": digest }),
        ),
        (
            "an upload of an API specification",
            "API_GATEWAY_API",
            "IMPORT_ASSET_FROM_API_GATEWAY_API",
            "ImportAssetFromApiGatewayApi",
            "ApiSpecificationUploadUrlExpiresAt",
            json!({
                "ApiId": "a1b2c3d4e5",
                "ApiName": "orders",
                "ApiSpecificationMd5Hash": digest,
                "ProtocolType": "REST",
                "Stage": "prod",
            }),
        ),
    ] {
        let (data_set_id, revision_id) =
            data_set_with_revision(&router, &format!("job-instants-{job_type}"), asset_type).await;
        let mut detail = detail;
        detail["DataSetId"] = json!(data_set_id);
        detail["RevisionId"] = json!(revision_id);

        let created = create_job(&router, job_type, json!({ member: detail }), None).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateJob, got {} body {:?}",
            created.status, created.body
        );
        let job_id = created.string("Id");
        let answered = date_time_members(case, &created, &["CreatedAt", "UpdatedAt"]);
        let created_at = date_time(case, "CreatedAt", created.member("CreatedAt"));
        let expires_at = date_time(
            case,
            expiry_member,
            &created.member("Details")[member][expiry_member],
        );
        assert!(
            expires_at.as_secs_f64() > created_at.as_secs_f64(),
            "case {case}: expected {expiry_member} after CreatedAt, got {:?} vs {:?}",
            created.member("Details")[member][expiry_member],
            created.member("CreatedAt")
        );

        let fetched = get_job(&router, &job_id).await;
        assert_eq!(
            date_time_members(case, &fetched, &["CreatedAt", "UpdatedAt"]),
            answered,
            "case {case}: expected GetJob to report the instants the create answered"
        );
        assert_eq!(
            fetched.member("Details")[member][expiry_member],
            created.member("Details")[member][expiry_member],
            "case {case}: expected GetJob to report the announced {expiry_member}"
        );
    }
}

/// RULE applied to an asset's list entry: an imported asset's `CreatedAt`/`UpdatedAt` are
/// `date-time` strings inside every `ListRevisionAssets` entry.
///
/// The asset is created the way a client creates one — `CreateJob` announces the upload
/// URL, the client PUTs the content, `StartJob` runs the import and the clock settles it —
/// so the instants under test are the ones the import recorded. Rows differ along the
/// content uploaded, because the instants are the same members whatever the bytes.
#[tokio::test]
async fn imported_asset_entries_carry_date_time_strings() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "asset-instants", "S3_SNAPSHOT").await;

    for (case, name, content) in [
        ("empty content", "empty.csv", Vec::new()),
        ("text content", "orders.csv", b"id,name\n1,ada\n".to_vec()),
        (
            "many kilobytes",
            "feed.ndjson",
            b"{\"row\":\"value\"}\n".repeat(1_024),
        ),
    ] {
        import_asset(
            &router,
            &engine,
            case,
            &data_set_id,
            &revision_id,
            name,
            &content,
        )
        .await;

        let listed = call(
            &router,
            "GET",
            &format!(
                "/v1/data-sets/{data_set_id}/revisions/{revision_id}/assets?maxResults={PAGE}"
            ),
            None,
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from ListRevisionAssets, got {} body {:?}",
            listed.status, listed.body
        );
        let entries = listed
            .member("Assets")
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an Assets list, got {:?}", listed.body))
            .clone();
        let entry = entries
            .iter()
            .find(|entry| entry["Name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("case {case}: expected an entry for {name} in {entries:?}"));
        let created_at = date_time(case, "CreatedAt", &entry["CreatedAt"]);
        let updated_at = date_time(case, "UpdatedAt", &entry["UpdatedAt"]);
        assert_eq!(
            created_at.as_secs_f64(),
            updated_at.as_secs_f64(),
            "case {case}: expected an untouched asset's instants to be equal, got {:?} and \
             {:?}",
            entry["CreatedAt"],
            entry["UpdatedAt"]
        );
    }
}

/// Import one asset into the revision through the calls a client makes.
async fn import_asset(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    data_set_id: &str,
    revision_id: &str,
    name: &str,
    content: &[u8],
) {
    let created = create_job(
        router,
        "IMPORT_ASSET_FROM_SIGNED_URL",
        json!({ "ImportAssetFromSignedUrl": {
            "AssetName": name,
            "DataSetId": data_set_id,
            "Md5Hash": base64_md5(content),
            "RevisionId": revision_id,
        }}),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected 201 from CreateJob, got {} body {:?}",
        created.status, created.body
    );
    let job_id = created.string("Id");
    let announced = created.member("Details")["ImportAssetFromSignedUrl"]["SignedUrl"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a SignedUrl in the answered details, got {:?}",
                created.member("Details")
            )
        })
        .to_string();
    let uploaded = put_upload(router, &announced, content).await;
    assert_eq!(
        uploaded, 200,
        "case {case}: expected 200 from the PUT to the announced URL, got {uploaded}"
    );
    let started = start_job(router, &job_id).await;
    assert_eq!(
        started.status, 202,
        "case {case}: expected 202 from StartJob, got {} body {:?}",
        started.status, started.body
    );
    settle_past_the_execution_span(engine);
    let settled = get_job(router, &job_id).await;
    assert_eq!(
        settled.string("State"),
        "COMPLETED",
        "case {case}: expected the import to complete, got {:?}",
        settled.body
    );
}

/// A data set of `asset_type` holding one revision.
async fn data_set_with_revision(
    router: &MultiServiceRouter,
    name: &str,
    asset_type: &str,
) -> (String, String) {
    let data_set_id = create_data_set(router, name, "for instants", asset_type, None)
        .await
        .string("Id");
    let revision = call(
        router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/revisions"),
        Some(json!({ "Comment": "for instants" })),
    )
    .await;
    assert_eq!(
        revision.status, 201,
        "case {name}: expected 201 from CreateRevision, got {} body {:?}",
        revision.status, revision.body
    );
    (data_set_id, revision.string("Id"))
}

/// RULE applied to event actions: `CreateEventAction`, `GetEventAction`,
/// `UpdateEventAction` and each `ListEventActions` entry answer `CreatedAt`/`UpdatedAt` as
/// `date-time` strings.
///
/// Rows differ along the shape of the `Action` the event action carries — a destination
/// bucket alone, one with a key pattern, and an encrypted export — because the instants are
/// the same members whatever the action.
#[tokio::test]
async fn event_action_answers_carry_date_time_strings() {
    let (router, engine) = harness();
    let data_set_id = entitled_data_set(&router, &engine, "event-action-instants").await;

    for (case, action) in [
        (
            "bucket only",
            json!({ "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "b" } } }),
        ),
        (
            "bucket and key pattern",
            json!({ "ExportRevisionToS3": { "RevisionDestination": {
                "Bucket": "b",
                "KeyPattern": "${Revision.CreatedAt}/${Asset.Name}",
            }}}),
        ),
        (
            "an encrypted export",
            json!({ "ExportRevisionToS3": {
                "RevisionDestination": { "Bucket": "b" },
                "Encryption": { "Type": "AES256" },
            }}),
        ),
    ] {
        let created = call(
            &router,
            "POST",
            "/v1/event-actions",
            Some(json!({
                "Event": { "RevisionPublished": { "DataSetId": data_set_id } },
                "Action": action,
            })),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateEventAction, got {} body {:?}",
            created.status, created.body
        );
        let event_action_id = created.string("Id");
        let answered = date_time_members(case, &created, &["CreatedAt", "UpdatedAt"]);

        let path = format!("/v1/event-actions/{event_action_id}");
        let fetched = call(&router, "GET", &path, None).await;
        assert_eq!(
            date_time_members(case, &fetched, &["CreatedAt", "UpdatedAt"]),
            answered,
            "case {case}: expected GetEventAction to report the instants the create answered"
        );

        let updated = call(
            &router,
            "PATCH",
            &path,
            Some(json!({ "Action": { "ExportRevisionToS3": {
                "RevisionDestination": { "Bucket": "renamed-bucket" },
            }}})),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateEventAction, got {} body {:?}",
            updated.status, updated.body
        );
        let after = date_time_members(case, &updated, &["CreatedAt", "UpdatedAt"]);
        assert_eq!(
            after[0], answered[0],
            "case {case}: expected CreatedAt to stay where the create left it"
        );

        let listed = call(
            &router,
            "GET",
            &format!("/v1/event-actions?maxResults={PAGE}"),
            None,
        )
        .await;
        let entry = entry_with_id(case, listed.member("EventActions"), &event_action_id);
        for member in ["CreatedAt", "UpdatedAt"] {
            date_time(case, member, &entry[member]);
        }
    }
}

/// An entitled data set, which is what an event action's event may watch.
///
/// No implemented operation mints one (AWS does it by accepting a data grant), so the state
/// is written through the shared engine, the way `event_actions.rs` does.
async fn entitled_data_set(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) -> String {
    let data_set_id = create_data_set(router, name, "e", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let mut store = state.lock();
    let data_set = store
        .data_sets
        .get_mut(&data_set_id)
        .expect("the data set the test just created");
    data_set.origin = dataexchange::state::ORIGIN_ENTITLED.to_string();
    data_set.origin_details = Some(shapes::OriginDetails {
        product_id: None,
        data_grant_id: Some("dgrant7890123456789012345678901".to_string()),
    });
    drop(store);
    data_set_id
}
