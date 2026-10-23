// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the job lifecycle: `CreateJob` and `GetJob`.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//! The harness (`emulator`, `harness`, `call`, `seconds`, `is_modelled_id`) is the
//! one `data_sets.rs` already uses.
//!
//! The tables span the class each rule generalises over — EVERY value of the model's
//! `Type` enum paired with the `Details` member the model pairs it with, the
//! mismatched pairings of that same catalog, and every way a named resource can be
//! absent — rather than the one recorded example.

use std::collections::HashSet;

use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange::state::ComAmazonawsDataexchangeDataexchangeState;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{
    call, create_data_set, emulator, harness, is_modelled_id, seconds, Answer, ACCOUNT, REGION,
};

/// A well-formed `Id` (`^[a-zA-Z0-9]{30,40}$`) for a resource this emulator does not
/// hold, so the answer is about the resource rather than about the request.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// An `AssetId` a job's details may name. Assets are minted by a job that runs, which
/// is `StartJob`'s business, so this is only a well-formed id for the detail member.
const ASSET_ID: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";

/// A value satisfying the model's `Md5Hash` / `ApiSpecificationMd5Hash` constraint
/// (`@length(min: 24, max: 24)` plus the base64 pattern): the base64 MD5 of no bytes.
const MD5_HASH: &str = "1B2M2Y8AsgTpgAmY7PhCfg==";

/// The host the harness sends every request to, and so the endpoint a URL this
/// service hands out must name.
fn endpoint() -> String {
    format!("dataexchange.{REGION}.amazonaws.com")
}

/// One row of the job catalog: a `Type`, the `Details` member the model pairs it
/// with, a well-formed body for that member, and whether the job would add assets to
/// the revision it names (an import, or the S3 data access create) rather than only
/// read it (an export).
///
/// Source: the model's `Type` enum and the members of `RequestDetails`, which carry
/// the same names as `ResponseDetails`'
/// (`smithy_query(query="shape", shape="Type,RequestDetails,ResponseDetails")`), and
/// which the handler's own `DETAILS_MEMBER_FOR_TYPE` table mirrors.
struct JobCase {
    job_type: &'static str,
    member: &'static str,
    detail: Value,
    writes_assets: bool,
}

/// The one `Type` whose `Details` NO request can carry: the model constrains its
/// `CatalogId` with the pattern `/^[\d]{12}$/`, whose literal `/` delimiters are
/// part of the pattern, so no value satisfies it and the boundary validator refuses
/// every such request before a handler sees it. Kept out of [`job_cases`] for that
/// reason and asserted on its own in
/// `lake_formation_tag_policy_jobs_are_refused_by_the_models_own_pattern`.
const LAKE_FORMATION_TYPE: &str = "IMPORT_ASSETS_FROM_LAKE_FORMATION_TAG_POLICY";

/// A `Details` body for [`LAKE_FORMATION_TYPE`] that is well formed in every respect
/// the model can satisfy.
fn lake_formation_detail(data_set_id: &str, revision_id: &str) -> Value {
    json!({ "ImportAssetsFromLakeFormationTagPolicy": {
        "CatalogId": ACCOUNT,
        "Database": {
            "Expression": [{ "TagKey": "tier", "TagValues": ["gold"] }],
            "Permissions": ["DESCRIBE"],
        },
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/dataexchange-lf"),
        "DataSetId": data_set_id,
        "RevisionId": revision_id,
    }})
}

/// Every `Type` the model admits and a request can carry, with the `Details` member
/// it names, against the data set `data_set_id` and its revision `revision_id`.
/// [`LAKE_FORMATION_TYPE`] is the one exception, and says why.
fn job_cases(data_set_id: &str, revision_id: &str) -> Vec<JobCase> {
    vec![
        JobCase {
            job_type: "CREATE_S3_DATA_ACCESS_FROM_S3_BUCKET",
            member: "CreateS3DataAccessFromS3Bucket",
            detail: json!({
                "AssetSource": {
                    "Bucket": "source-bucket",
                    "KeyPrefixes": ["prefix/"],
                    "Keys": ["prefix/object.csv"],
                },
                "DataSetId": data_set_id,
                "RevisionId": revision_id,
            }),
            writes_assets: true,
        },
        JobCase {
            job_type: "EXPORT_ASSETS_TO_S3",
            member: "ExportAssetsToS3",
            detail: json!({
                "AssetDestinations": [
                    { "AssetId": ASSET_ID, "Bucket": "destination-bucket", "Key": "out.csv" },
                ],
                "DataSetId": data_set_id,
                "Encryption": { "Type": "AES256" },
                "RevisionId": revision_id,
            }),
            writes_assets: false,
        },
        JobCase {
            job_type: "EXPORT_ASSET_TO_SIGNED_URL",
            member: "ExportAssetToSignedUrl",
            detail: json!({
                "AssetId": ASSET_ID,
                "DataSetId": data_set_id,
                "RevisionId": revision_id,
            }),
            writes_assets: false,
        },
        JobCase {
            job_type: "EXPORT_REVISIONS_TO_S3",
            member: "ExportRevisionsToS3",
            detail: json!({
                "DataSetId": data_set_id,
                "RevisionDestinations": [
                    {
                        "Bucket": "destination-bucket",
                        "KeyPattern": "${Revision.id}/${Asset.name}",
                        "RevisionId": revision_id,
                    },
                ],
            }),
            writes_assets: false,
        },
        JobCase {
            job_type: "IMPORT_ASSETS_FROM_REDSHIFT_DATA_SHARES",
            member: "ImportAssetsFromRedshiftDataShares",
            detail: json!({
                "AssetSources": [
                    {
                        "DataShareArn":
                            format!("arn:aws:redshift:{REGION}:{ACCOUNT}:datashare:cluster/share"),
                    },
                ],
                "DataSetId": data_set_id,
                "RevisionId": revision_id,
            }),
            writes_assets: true,
        },
        JobCase {
            job_type: "IMPORT_ASSETS_FROM_S3",
            member: "ImportAssetsFromS3",
            detail: json!({
                "AssetSources": [{ "Bucket": "source-bucket", "Key": "object.csv" }],
                "DataSetId": data_set_id,
                "RevisionId": revision_id,
            }),
            writes_assets: true,
        },
        JobCase {
            job_type: "IMPORT_ASSET_FROM_API_GATEWAY_API",
            member: "ImportAssetFromApiGatewayApi",
            detail: json!({
                "ApiDescription": "an imported REST API",
                "ApiId": "a1b2c3d4e5",
                "ApiKey": "a-key",
                "ApiName": "orders",
                "ApiSpecificationMd5Hash": MD5_HASH,
                "DataSetId": data_set_id,
                "ProtocolType": "REST",
                "RevisionId": revision_id,
                "Stage": "prod",
            }),
            writes_assets: true,
        },
        JobCase {
            job_type: "IMPORT_ASSET_FROM_SIGNED_URL",
            member: "ImportAssetFromSignedUrl",
            detail: json!({
                "AssetName": "object.csv",
                "DataSetId": data_set_id,
                "Md5Hash": MD5_HASH,
                "RevisionId": revision_id,
            }),
            writes_assets: true,
        },
    ]
}

/// A data set holding one revision, as every job in these tables names one.
async fn data_set_with_revision(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let data_set = create_data_set(router, name, "for jobs", "S3_SNAPSHOT", None).await;
    assert_eq!(
        data_set.status, 201,
        "case {name}: expected 201 from CreateDataSet, got {} body {:?}",
        data_set.status, data_set.body
    );
    let data_set_id = data_set.string("Id");
    let revision = call(
        router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/revisions"),
        Some(json!({ "Comment": "for jobs" })),
    )
    .await;
    assert_eq!(
        revision.status, 201,
        "case {name}: expected 201 from CreateRevision, got {} body {:?}",
        revision.status, revision.body
    );
    (data_set_id, revision.string("Id"))
}

async fn create_job(
    router: &MultiServiceRouter,
    job_type: &str,
    details: Value,
    asset_configuration: Option<Value>,
) -> Answer {
    let mut body = json!({ "Type": job_type, "Details": details });
    if let Some(configuration) = asset_configuration {
        body["AssetConfiguration"] = configuration;
    }
    call(router, "POST", "/v1/jobs", Some(body)).await
}

/// How many jobs the store holds, so a refusal can be shown to have persisted
/// nothing.
fn stored_jobs(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<ComAmazonawsDataexchangeDataexchangeState>()
        .lock()
        .jobs
        .len()
}

/// RULE: `CreateJob` records the job described by `Type` and the one `Details` member
/// that `Type` names — a freshly minted `Id`, an `Arn` built from it, `State`
/// `WAITING` because nothing has started the job, those details in their response
/// form, the asset configuration as given, no errors and equal timestamps — and
/// `GetJob` answers out of that same stored record. This holds for every `Type` the
/// model admits.
#[tokio::test]
async fn create_then_get_round_trips_every_job_type() {
    let router = emulator();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "every-job-type").await;
    let mut minted: HashSet<String> = HashSet::new();

    for (index, case) in job_cases(&data_set_id, &revision_id).into_iter().enumerate() {
        let label = case.job_type;
        // Rows differ along the job type, the details member and — so the echo is
        // exercised both ways — whether an asset configuration was asked for.
        let configuration = match index % 2 {
            0 => None,
            _ => Some(json!({ "Tags": [{ "Key": "team", "Value": "data" }] })),
        };
        let created = create_job(
            &router,
            case.job_type,
            json!({ case.member: case.detail.clone() }),
            configuration.clone(),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {label}: expected 201 from CreateJob, got {} body {:?}",
            created.status, created.body
        );

        let id = created.string("Id");
        assert!(
            is_modelled_id(&id),
            "case {label}: expected Id to match ^[a-zA-Z0-9]{{30,40}}$, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {label}: expected a freshly minted Id, got the already-issued {id:?}"
        );
        assert_eq!(
            created.string("Arn"),
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:jobs/{id}"),
            "case {label}: expected the Arn to be derived from the region, account and Id"
        );
        assert_eq!(
            created.string("Type"),
            case.job_type,
            "case {label}: expected the requested Type back"
        );
        assert_eq!(
            created.string("State"),
            "WAITING",
            "case {label}: expected State=WAITING on a job nothing has started, got {:?}",
            created.member("State")
        );
        assert!(
            created.member("Errors").is_null(),
            "case {label}: expected no Errors on a job that has not run, got {:?}",
            created.member("Errors")
        );
        let created_at = seconds(created.member("CreatedAt"), label);
        assert_eq!(
            created_at,
            seconds(created.member("UpdatedAt"), label),
            "case {label}: expected CreatedAt == UpdatedAt on a freshly created job"
        );
        match &configuration {
            None => assert!(
                created.member("AssetConfiguration").is_null(),
                "case {label}: expected no AssetConfiguration when none was asked for, got {:?}",
                created.member("AssetConfiguration")
            ),
            Some(expected) => assert_eq!(
                created.member("AssetConfiguration"),
                expected,
                "case {label}: expected the requested AssetConfiguration back"
            ),
        }

        // The details come back under exactly the member `Type` names, and nothing
        // else.
        let details = created
            .member("Details")
            .as_object()
            .unwrap_or_else(|| panic!("case {label}: Details is not an object: {:?}", created.body));
        assert_eq!(
            details.keys().collect::<Vec<_>>(),
            vec![case.member],
            "case {label}: expected Details to carry only {}, got {:?}",
            case.member,
            details.keys().collect::<Vec<_>>()
        );
        let answered = &details[case.member];
        // Every member the caller sent comes back unchanged.
        for (member, requested) in case
            .detail
            .as_object()
            .expect("the table's details are objects")
        {
            assert_eq!(
                answered.get(member),
                Some(requested),
                "case {label}: expected {member} echoed as {requested:?}, got {:?}",
                answered.get(member)
            );
        }

        // A URL this service hands out names the endpoint the request arrived at and
        // expires after the job was created; it is minted only for the upload an
        // import takes its content from.
        for (url_member, expiry_member) in [
            ("SignedUrl", "SignedUrlExpiresAt"),
            (
                "ApiSpecificationUploadUrl",
                "ApiSpecificationUploadUrlExpiresAt",
            ),
        ] {
            let Some(url) = answered.get(url_member).and_then(Value::as_str) else {
                continue;
            };
            let expected_prefix = format!("https://{}/v1/jobs/{id}/", endpoint());
            assert!(
                url.starts_with(&expected_prefix),
                "case {label}: expected {url_member} under {expected_prefix:?}, got {url:?}"
            );
            let expires = seconds(
                answered
                    .get(expiry_member)
                    .unwrap_or_else(|| panic!("case {label}: {url_member} without {expiry_member}")),
                label,
            );
            assert!(
                expires > created_at,
                "case {label}: expected {expiry_member} after CreatedAt, got {expires} vs {created_at}"
            );
        }
        // An import that uploads its content needs that URL at create time; an export
        // has nothing to hand out yet, because nothing has been exported.
        assert_eq!(
            answered.get("ApiSpecificationUploadUrl").is_some(),
            case.member == "ImportAssetFromApiGatewayApi",
            "case {label}: expected ApiSpecificationUploadUrl only for the API Gateway import, \
             got {:?}",
            answered.get("ApiSpecificationUploadUrl")
        );
        assert_eq!(
            answered.get("SignedUrl").is_some(),
            case.member == "ImportAssetFromSignedUrl",
            "case {label}: expected SignedUrl only for the signed-URL import, got {:?}",
            answered.get("SignedUrl")
        );

        let fetched = call(&router, "GET", &format!("/v1/jobs/{id}"), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {label}: expected 200 from GetJob, got {} body {:?}",
            fetched.status, fetched.body
        );
        for member in [
            "Arn",
            "AssetConfiguration",
            "CreatedAt",
            "Details",
            "Errors",
            "Id",
            "State",
            "Type",
            "UpdatedAt",
        ] {
            assert_eq!(
                fetched.member(member),
                created.member(member),
                "case {label}: expected GetJob to report the persisted {member}, got {:?} \
                 after a create that answered {:?}",
                fetched.member(member),
                created.member(member)
            );
        }
    }
}

/// RULE: `Type` names exactly one `Details` member, so a request whose `Details` is
/// not exactly that member — another type's member, no member, or two at once — is
/// refused as a 400 `ValidationException` and persists nothing.
#[tokio::test]
async fn details_that_do_not_match_the_type_are_refused() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "mismatched-details").await;
    let cases = job_cases(&data_set_id, &revision_id);

    // Rows: every type paired with the NEXT type's details member (so each member in
    // the catalog is wrong for some type), then the two degenerate shapes.
    let mut rows: Vec<(String, &str, Value)> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let other = &cases[(index + 1) % cases.len()];
        rows.push((
            format!("{} carrying {}", case.job_type, other.member),
            case.job_type,
            json!({ other.member: other.detail.clone() }),
        ));
    }
    rows.push((
        "details carrying no member".to_string(),
        cases[0].job_type,
        json!({}),
    ));
    rows.push((
        format!("{} carrying two members", cases[0].job_type),
        cases[0].job_type,
        json!({
            cases[0].member: cases[0].detail.clone(),
            cases[1].member: cases[1].detail.clone(),
        }),
    ));
    // The one type a request cannot carry the details of is still refused when its
    // `Details` names another type's member: the disagreement is settled before any
    // of that member's own constraints matter.
    rows.push((
        format!("{LAKE_FORMATION_TYPE} carrying {}", cases[0].member),
        LAKE_FORMATION_TYPE,
        json!({ cases[0].member: cases[0].detail.clone() }),
    ));

    for (label, job_type, details) in rows {
        let before = stored_jobs(&engine);
        let refused = create_job(&router, job_type, details, None).await;
        assert_eq!(
            refused.status, 400,
            "case {label}: expected 400, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {label}: expected ValidationException, got {:?}",
            refused.error
        );
        assert_eq!(
            stored_jobs(&engine),
            before,
            "case {label}: expected a refused CreateJob to persist nothing"
        );
    }
}

/// RULE: a job names the resources it will work on, so a `Details` naming a data set
/// or a revision this emulator does not hold is refused as a 404
/// `ResourceNotFoundException` reporting which resource was missing — whichever job
/// type named it.
#[tokio::test]
async fn a_job_naming_an_absent_resource_is_not_found() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "absent-resources").await;

    // Rows: the data set absent, the revision absent, and — for the one job type
    // whose details name several revisions — one present and one absent.
    let mut rows: Vec<(String, &str, Value, &str)> = Vec::new();
    for case in job_cases(ABSENT_ID, revision_id.as_str()) {
        rows.push((
            format!("{} naming an absent data set", case.job_type),
            case.job_type,
            json!({ case.member: case.detail }),
            "DATA_SET",
        ));
    }
    for case in job_cases(data_set_id.as_str(), ABSENT_ID) {
        rows.push((
            format!("{} naming an absent revision", case.job_type),
            case.job_type,
            json!({ case.member: case.detail }),
            "REVISION",
        ));
    }
    rows.push((
        "EXPORT_REVISIONS_TO_S3 naming one present and one absent revision".to_string(),
        "EXPORT_REVISIONS_TO_S3",
        json!({
            "ExportRevisionsToS3": {
                "DataSetId": data_set_id,
                "RevisionDestinations": [
                    { "Bucket": "b", "RevisionId": revision_id },
                    { "Bucket": "b", "RevisionId": ABSENT_ID },
                ],
            },
        }),
        "REVISION",
    ));

    for (label, job_type, details, resource_type) in rows {
        let before = stored_jobs(&engine);
        let refused = create_job(&router, job_type, details, None).await;
        assert_eq!(
            refused.status, 404,
            "case {label}: expected 404, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label}: expected ResourceNotFoundException, got {:?}",
            refused.error
        );
        assert_eq!(
            refused.member("ResourceId").as_str(),
            Some(ABSENT_ID),
            "case {label}: expected the absent id reported, got {:?}",
            refused.member("ResourceId")
        );
        assert_eq!(
            refused.member("ResourceType").as_str(),
            Some(resource_type),
            "case {label}: expected ResourceType={resource_type}, got {:?}",
            refused.member("ResourceType")
        );
        assert_eq!(
            stored_jobs(&engine),
            before,
            "case {label}: expected a refused CreateJob to persist nothing"
        );
    }
}

/// RULE: a finalized revision is read-only (the model's own `Finalized`
/// documentation calls it "this read-only state"), so a job that would ADD assets to
/// one conflicts with its state and is refused as a 409 `ConflictException`, while a
/// job that only READS that revision is accepted.
///
/// Finalizing is `UpdateRevision`'s job and that operation is still a scaffold, so
/// the revision is finalized in state here rather than over the wire.
#[tokio::test]
async fn a_job_adding_assets_to_a_finalized_revision_conflicts() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "finalized").await;
    {
        let state = engine.state::<ComAmazonawsDataexchangeDataexchangeState>();
        let mut store = state.lock();
        let data_set = store
            .data_sets
            .get_mut(&data_set_id)
            .expect("the data set this test created");
        data_set
            .revisions
            .get_mut(&revision_id)
            .expect("the revision this test created")
            .finalized = true;
    }

    for case in job_cases(&data_set_id, &revision_id) {
        let label = case.job_type;
        let answer = create_job(
            &router,
            case.job_type,
            json!({ case.member: case.detail }),
            None,
        )
        .await;
        if case.writes_assets {
            assert_eq!(
                answer.status, 409,
                "case {label}: expected 409 for a job adding assets to a finalized revision, \
                 got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ConflictException"),
                "case {label}: expected ConflictException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.member("ResourceId").as_str(),
                Some(revision_id.as_str()),
                "case {label}: expected the conflicting revision reported, got {:?}",
                answer.member("ResourceId")
            );
            assert_eq!(
                answer.member("ResourceType").as_str(),
                Some("REVISION"),
                "case {label}: expected ResourceType=REVISION, got {:?}",
                answer.member("ResourceType")
            );
        } else {
            assert_eq!(
                answer.status, 201,
                "case {label}: expected 201 for a job that only reads a finalized revision, \
                 got {} body {:?}",
                answer.status, answer.body
            );
        }
    }
}

/// RULE: a caller-sized detail list the model does not bound is still bounded by the
/// store: the bound itself is accepted and one entry past it is refused as a 400
/// `ValidationException`.
#[tokio::test]
async fn caller_sized_detail_lists_are_bounded() {
    use service_com_amazonaws_dataexchange_dataexchange::state::MAX_JOB_DETAIL_ENTRIES;

    let router = emulator();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "bounded-details").await;

    /// A `Details` body for one job type as a function of its list's entry count.
    type DetailsOfSize = fn(usize, &str, &str) -> Value;

    // Rows: one list per job type that carries one, as a function of its entry count.
    let rows: Vec<(&str, &str, DetailsOfSize)> = vec![
        (
            "IMPORT_ASSETS_FROM_S3",
            "ImportAssetsFromS3.AssetSources",
            |count, data_set_id, revision_id| {
                json!({ "ImportAssetsFromS3": {
                    "AssetSources": (0..count)
                        .map(|index| json!({ "Bucket": "b", "Key": format!("k{index}") }))
                        .collect::<Vec<_>>(),
                    "DataSetId": data_set_id,
                    "RevisionId": revision_id,
                }})
            },
        ),
        (
            "EXPORT_ASSETS_TO_S3",
            "ExportAssetsToS3.AssetDestinations",
            |count, data_set_id, revision_id| {
                json!({ "ExportAssetsToS3": {
                    "AssetDestinations": (0..count)
                        .map(|_| json!({ "AssetId": ASSET_ID, "Bucket": "b" }))
                        .collect::<Vec<_>>(),
                    "DataSetId": data_set_id,
                    "RevisionId": revision_id,
                }})
            },
        ),
        (
            "EXPORT_REVISIONS_TO_S3",
            "ExportRevisionsToS3.RevisionDestinations",
            |count, data_set_id, revision_id| {
                json!({ "ExportRevisionsToS3": {
                    "DataSetId": data_set_id,
                    "RevisionDestinations": (0..count)
                        .map(|_| json!({ "Bucket": "b", "RevisionId": revision_id }))
                        .collect::<Vec<_>>(),
                }})
            },
        ),
        (
            "IMPORT_ASSETS_FROM_REDSHIFT_DATA_SHARES",
            "ImportAssetsFromRedshiftDataShares.AssetSources",
            |count, data_set_id, revision_id| {
                json!({ "ImportAssetsFromRedshiftDataShares": {
                    "AssetSources": (0..count)
                        .map(|index| json!({
                            "DataShareArn":
                                format!("arn:aws:redshift:{REGION}:{ACCOUNT}:datashare:c/s{index}"),
                        }))
                        .collect::<Vec<_>>(),
                    "DataSetId": data_set_id,
                    "RevisionId": revision_id,
                }})
            },
        ),
        (
            "CREATE_S3_DATA_ACCESS_FROM_S3_BUCKET",
            "CreateS3DataAccessFromS3Bucket.AssetSource.Keys",
            |count, data_set_id, revision_id| {
                json!({ "CreateS3DataAccessFromS3Bucket": {
                    "AssetSource": {
                        "Bucket": "b",
                        "Keys": (0..count).map(|index| json!(format!("k{index}"))).collect::<Vec<_>>(),
                    },
                    "DataSetId": data_set_id,
                    "RevisionId": revision_id,
                }})
            },
        ),
    ];

    for (job_type, member, details) in rows {
        let accepted = create_job(
            &router,
            job_type,
            details(MAX_JOB_DETAIL_ENTRIES, &data_set_id, &revision_id),
            None,
        )
        .await;
        assert_eq!(
            accepted.status, 201,
            "case {member} at the bound of {MAX_JOB_DETAIL_ENTRIES}: expected 201, got {} \
             body {:?}",
            accepted.status, accepted.body
        );

        let refused = create_job(
            &router,
            job_type,
            details(MAX_JOB_DETAIL_ENTRIES + 1, &data_set_id, &revision_id),
            None,
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {member} one past the bound: expected 400, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {member} one past the bound: expected ValidationException, got {:?}",
            refused.error
        );
        assert!(
            refused
                .member("Message")
                .as_str()
                .is_some_and(|message| message.contains(member.rsplit('.').next().unwrap_or(member))),
            "case {member} one past the bound: expected the message to name the member, got {:?}",
            refused.member("Message")
        );
    }
}

/// RULE: an `AssetConfiguration` tag key or value past AWS's tagging limits is
/// refused as a 400 `ValidationException`; the limits themselves are accepted.
#[tokio::test]
async fn asset_configuration_tags_are_bounded() {
    use service_com_amazonaws_dataexchange_dataexchange::state::{
        MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
    };

    let router = emulator();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "bounded-tags").await;
    let details = json!({ "ImportAssetsFromS3": {
        "AssetSources": [{ "Bucket": "b", "Key": "k" }],
        "DataSetId": data_set_id,
        "RevisionId": revision_id,
    }});

    // Rows: each tagging bound, at the bound and one character past it.
    let rows = [
        ("tag key", MAX_TAG_KEY_CHARS, true),
        ("tag value", MAX_TAG_VALUE_CHARS, false),
    ];
    for (label, bound, on_key) in rows {
        for (case, length) in [("at the bound", bound), ("past the bound", bound + 1)] {
            let tag = if on_key {
                json!({ "Key": "k".repeat(length), "Value": "v" })
            } else {
                json!({ "Key": "k", "Value": "v".repeat(length) })
            };
            let answer = create_job(
                &router,
                "IMPORT_ASSETS_FROM_S3",
                details.clone(),
                Some(json!({ "Tags": [tag] })),
            )
            .await;
            let expected = if length > bound { 400 } else { 201 };
            assert_eq!(
                answer.status, expected,
                "case {label} {case}: expected {expected}, got {} body {:?}",
                answer.status, answer.body
            );
        }
    }
}

/// RULE: a modelled constraint is enforced before a handler runs, so a `Details`
/// member whose own pattern admits no value at all is refused as a 400
/// `ValidationException` naming that member — this model gives
/// `ImportAssetsFromLakeFormationTagPolicy.CatalogId` the pattern `/^[\d]{12}$/`,
/// whose `/` delimiters are part of the pattern, so no `CatalogId` satisfies it.
#[tokio::test]
async fn lake_formation_tag_policy_jobs_are_refused_by_the_models_own_pattern() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "lake-formation").await;

    // Rows: a 12-digit account id, and the same id with the pattern's own delimiters
    // written out — neither satisfies a pattern that admits nothing.
    for (label, catalog_id) in [
        ("twelve digits", ACCOUNT.to_string()),
        ("the pattern's delimiters written out", format!("/{ACCOUNT}/")),
    ] {
        let mut details = lake_formation_detail(&data_set_id, &revision_id);
        details["ImportAssetsFromLakeFormationTagPolicy"]["CatalogId"] = json!(catalog_id);
        let refused = create_job(&router, LAKE_FORMATION_TYPE, details, None).await;
        assert_eq!(
            refused.status, 400,
            "case {label}: expected 400, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {label}: expected ValidationException, got {:?}",
            refused.error
        );
        assert_eq!(
            stored_jobs(&engine),
            0,
            "case {label}: expected a refused CreateJob to persist nothing"
        );
    }
}

/// RULE: a well-formed `JobId` that names no stored job is a 404
/// `ResourceNotFoundException` reporting the job id, while a `JobId` outside the
/// modelled `^[a-zA-Z0-9]{30,40}$` pattern is a 400 `ValidationException` — the
/// request is malformed, not the resource absent.
#[tokio::test]
async fn get_job_answers_404_for_an_unknown_id_and_400_for_a_malformed_one() {
    let router = emulator();

    // Rows differ along HOW the id breaks (or satisfies) the modelled pattern.
    let ids = [
        ("too short", "a".repeat(29)),
        ("at the minimum length", "a".repeat(30)),
        ("at the maximum length", "a".repeat(40)),
        ("too long", "a".repeat(41)),
        (
            "a hyphen is not alphanumeric",
            format!("{}-{}", "a".repeat(16), "b".repeat(15)),
        ),
    ];

    for (label, id) in ids {
        let answer = call(&router, "GET", &format!("/v1/jobs/{id}"), None).await;
        if is_modelled_id(&id) {
            assert_eq!(
                answer.status, 404,
                "case {label}: expected 404 for the well-formed but unknown {id:?}, got {} \
                 body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {label}: expected ResourceNotFoundException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.member("ResourceId").as_str(),
                Some(id.as_str()),
                "case {label}: expected the requested JobId reported, got {:?}",
                answer.member("ResourceId")
            );
            assert_eq!(
                answer.member("ResourceType").as_str(),
                Some("JOB"),
                "case {label}: expected ResourceType=JOB, got {:?}",
                answer.member("ResourceType")
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {label}: expected 400 for {id:?}, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {label}: expected ValidationException for {id:?}, got {:?}",
                answer.error
            );
        }
    }
}

/// RULE: `GetJob` reports, so reading a job neither changes it nor changes what a
/// later reader sees — the same job read twice answers the same bytes.
#[tokio::test]
async fn reading_a_job_changes_nothing() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) = data_set_with_revision(&router, "stable-reads").await;

    for case in job_cases(&data_set_id, &revision_id) {
        let label = case.job_type;
        let created = create_job(
            &router,
            case.job_type,
            json!({ case.member: case.detail }),
            None,
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {label}: expected 201 from CreateJob, got {} body {:?}",
            created.status, created.body
        );
        let id = created.string("Id");
        let before = stored_jobs(&engine);
        let first = call(&router, "GET", &format!("/v1/jobs/{id}"), None).await;
        let second = call(&router, "GET", &format!("/v1/jobs/{id}"), None).await;
        assert_eq!(
            first.body, second.body,
            "case {label}: expected two reads of one job to answer the same, got {:?} then {:?}",
            first.body, second.body
        );
        assert_eq!(
            stored_jobs(&engine),
            before,
            "case {label}: expected GetJob to persist nothing"
        );
    }
}
