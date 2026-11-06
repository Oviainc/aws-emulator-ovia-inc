// SPDX-License-Identifier: CC0-1.0

//! `CreateHarvestJob` pre-flights the DESTINATION of the harvest it is asked to carry out.
//!
//! THE RULE UNDER TEST. A harvest PRODUCES an object: `S3Destination$BucketName` is "The
//! name of an S3 bucket within which harvested content will be exported",
//! `S3Destination$ManifestKey` "The key in the specified S3 bucket where the harvested
//! top-level manifest will be placed" and `S3Destination$RoleArn` "The IAM role used to
//! write to the specified S3 bucket". So a submission is accepted only when the harvested
//! manifest could be written where it says — the bucket is a name Amazon S3 holds a bucket
//! under AND one the account holds, and the role is one MediaPackage could act as — and a
//! submission whose destination fails any of that is refused with the modelled
//! `UnprocessableEntityException` (422), keyed by the member the caller has to fix, with NO
//! job persisted: the following `DescribeHarvestJob` is a 404 and `ListHarvestJobs` has no
//! such entry.
//!
//! What the pre-flight is NOT is a write: it reads, so the destination it resolves is left
//! exactly as the caller left it, and the harvested manifest appears only once the clock
//! makes the export due (`tests/all/harvest_export.rs`).
//!
//! Every case drives real HTTP through the `MultiServiceRouter` over ONE engine carrying
//! this service's store, Amazon S3's and IAM's — the way the assembled emulator runs them —
//! so every destination bucket is created with a real `CreateBucket` and every role with a
//! real `CreateRole`.
//!
//! The rows differ along what the rule generalises over: the bucket NAME's form (hyphenated,
//! dotted, digits, and the published length bounds with one character past each), which
//! published naming rule a refused name breaks, whether the named bucket is one the account
//! holds, the shape of the manifest key, and which role the destination names (one this
//! account holds and trusts this service, one of another account, one of this account that
//! IAM holds nothing for, one whose trust policy withholds the grant).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, error_code, ACCOUNT, OTHER_ACCOUNT};
use crate::harvest_export::{create_bucket, get_object};
use crate::harvest_job_rejections::{create_role, policy, PRINCIPAL};
use crate::harvest_jobs::hold_transitions_open;

/// The channel every endpoint below packages.
const CHANNEL_ID: &str = "destination-channel";

/// The endpoint every submission harvests: it retains a startover window, so the endpoint
/// precondition holds and the destination is what each case is about.
const ENDPOINT_ID: &str = "destination-endpoint";

/// A second endpoint that retains NO startover window, for the case that fails both the
/// endpoint precondition and the destination.
const CLOSED_ENDPOINT_ID: &str = "destination-endpoint-closed";

/// The member a refusal about the destination bucket is keyed by.
const BUCKET_MEMBER: &str = "s3Destination.bucketName";

/// The member a refusal about the destination role is keyed by.
const ROLE_MEMBER: &str = "s3Destination.roleArn";

/// The published floor and ceiling of a general-purpose bucket NAME, read off the shared
/// rule every service that binds a caller-supplied bucket name uses, so the rows below state
/// the bound once.
const MIN_BUCKET_NAME_CHARS: usize = service_common::s3_bucket_name::MIN_BUCKET_NAME_CHARS;
/// See [`MIN_BUCKET_NAME_CHARS`].
const MAX_BUCKET_NAME_CHARS: usize = service_common::s3_bucket_name::MAX_BUCKET_NAME_CHARS;

/// One emulator: this service, Amazon S3 and IAM on a single shared engine behind the
/// production router — what `core/router` assembles, narrowed to the three services the rule
/// under test spans.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// A trust policy that lets MediaPackage assume the role.
fn assumable() -> String {
    policy("Allow", "\"sts:AssumeRole\"", &format!("\"{PRINCIPAL}\""))
}

/// Create the channel and the two origin endpoints every case below harvests from.
async fn provision(router: &MultiServiceRouter) {
    create_channel(router, CHANNEL_ID).await;
    for (id, startover) in [
        (ENDPOINT_ID, Some(86_400)),
        (CLOSED_ENDPOINT_ID, None::<i64>),
    ] {
        let mut request = json!({ "id": id, "channelId": CHANNEL_ID, "hlsPackage": {} });
        if let Some(seconds) = startover {
            request["startoverWindowSeconds"] = json!(seconds);
        }
        let (status, body) = call(router, "POST", "/origin_endpoints", Some(&request)).await;
        assert_eq!(
            status, 200,
            "creating the fixture endpoint {id:?} expected 200, got {status} {body:?}"
        );
    }
}

/// A `CreateHarvestJob` body naming `endpoint`, `bucket`, `key` and `role`.
fn submission(id: &str, endpoint: &str, bucket: &str, key: &str, role: &str) -> Value {
    json!({
        "id": id,
        "originEndpointId": endpoint,
        "startTime": "2026-01-01T00:00:00Z",
        "endTime": "2026-01-01T01:00:00Z",
        "s3Destination": {
            "bucketName": bucket,
            "manifestKey": key,
            "roleArn": role
        }
    })
}

/// Submit a harvest job.
async fn submit(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/harvest_jobs", Some(body)).await
}

/// Read a harvest job back.
async fn describe(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/harvest_jobs/{id}"), None).await
}

/// Every id `ListHarvestJobs` currently reports.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "GET", "/harvest_jobs", None).await;
    assert_eq!(
        status, 200,
        "ListHarvestJobs expected 200, got {status} {body:?}"
    );
    body.get("harvestJobs")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a ListHarvestJobs body carries harvestJobs, got {body:?}"))
        .iter()
        .map(|job| {
            job.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert `refusal` is this service's 422 stating `requirement` for `member`, and that no
/// job was persisted under `id` — neither readable nor listed.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    refusal: (u16, Value),
    member: &str,
    requirement: &str,
) {
    let (status, body) = refusal;
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body:?}"
    );
    assert_eq!(
        body.pointer(&format!("/errors/{member}")),
        Some(&json!(requirement)),
        "case {case:?}: expected the refusal keyed by {member:?} stating {requirement:?}, got \
         {body:?}"
    );
    assert_eq!(
        body.get("message"),
        Some(&json!(format!("{member} {requirement}"))),
        "case {case:?}: the sentence must state the same thing as the keyed reason, got {body:?}"
    );

    let (status, described) = describe(router, id).await;
    assert_eq!(
        status, 404,
        "case {case:?}: a refused submission must persist no job, got {status} {described:?}"
    );
    assert_eq!(
        error_code(&described),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {described:?}"
    );
    let ids = listed_ids(router).await;
    assert!(
        !ids.contains(&id.to_string()),
        "case {case:?}: a refused submission must not be listed, got {ids:?}"
    );
}

/// Which role a submission names.
enum Role {
    /// A role this account holds, whose trust policy lets MediaPackage assume it.
    Assumable,
    /// A role of ANOTHER account, which this emulator holds no record of.
    AnotherAccount,
}

/// One accepted destination: the bucket to create and name, the key, and the role.
struct Accepted {
    case: &'static str,
    id: &'static str,
    bucket: String,
    manifest_key: &'static str,
    role: Role,
}

/// The destinations a submission is accepted with, spanning the bucket-name forms Amazon S3
/// holds a bucket under — including both published length bounds — the shape of the manifest
/// key, and both kinds of role the service cannot rule out.
fn accepted() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "a hyphenated bucket, a nested key, a role this account holds",
            id: "accepted-hyphenated",
            bucket: "destination-harvests".to_string(),
            manifest_key: "harvests/2026/01/01/index.m3u8",
            role: Role::Assumable,
        },
        Accepted {
            case: "a dotted bucket, a bare key, a role of another account",
            id: "accepted_dotted",
            bucket: "destination.harvests.two".to_string(),
            manifest_key: "index.m3u8",
            role: Role::AnotherAccount,
        },
        Accepted {
            case: "a bucket of digits, a dotted key",
            id: "Accepted3",
            bucket: "20260101123456".to_string(),
            manifest_key: "replay/live.ism/Manifest",
            role: Role::Assumable,
        },
        Accepted {
            case: "a bucket name at the published floor",
            id: "accepted-floor",
            bucket: "d3h".to_string(),
            manifest_key: "floor/index.mpd",
            role: Role::Assumable,
        },
        Accepted {
            case: "a bucket name at the published ceiling",
            id: "accepted-ceiling",
            // The ceiling exactly: a leading letter, then filler, as the naming rules allow.
            bucket: format!("d{}", "x".repeat(MAX_BUCKET_NAME_CHARS - 1)),
            manifest_key: "ceiling/index.m3u8",
            role: Role::AnotherAccount,
        },
    ]
}

#[tokio::test]
async fn a_destination_the_harvest_could_be_written_to_is_accepted_and_read_back() {
    let (router, engine) = emulator();
    provision(&router).await;
    let held_role = create_role(&router, "mediapackage-destination", &assumable()).await;
    let foreign_role = format!("arn:aws:iam::{OTHER_ACCOUNT}:role/HarvestRole");

    for row in accepted() {
        let case = row.case;
        create_bucket(&router, &row.bucket).await;
        let role = match row.role {
            Role::Assumable => held_role.clone(),
            Role::AnotherAccount => foreign_role.clone(),
        };
        let (status, created) = submit(
            &router,
            &submission(row.id, ENDPOINT_ID, &row.bucket, row.manifest_key, &role),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateHarvestJob expected 200, got {status} {created:?}"
        );
        // The owed harvest is held open, so the reads below see the record the submit wrote
        // rather than one the clock has since moved.
        hold_transitions_open(&engine);

        // The destination is reported as sent, and the job is readable back: the response is
        // the stored record.
        assert_eq!(
            created.pointer("/s3Destination/bucketName"),
            Some(&json!(row.bucket)),
            "case {case:?}: the destination bucket must be reported as sent, got {created:?}"
        );
        assert_eq!(
            created.pointer("/s3Destination/roleArn"),
            Some(&json!(role)),
            "case {case:?}: the destination role must be reported as sent, got {created:?}"
        );
        let (status, described) = describe(&router, row.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeHarvestJob expected 200, got {status} {described:?}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: the read must render the stored record, got {described:?}"
        );

        // The pre-flight READ the destination: nothing has been written to it, because the
        // export is the work the clock owes rather than something the submit did.
        let (absent, _, _) = get_object(&router, &row.bucket, row.manifest_key).await;
        assert_eq!(
            absent, 404,
            "case {case:?}: the pre-flight must leave the destination untouched, got {absent}"
        );
    }

    // Every accepted submission, and nothing else, is in the listing.
    let mut expected: Vec<String> = accepted().iter().map(|row| row.id.to_string()).collect();
    expected.sort();
    let ids = listed_ids(&router).await;
    assert_eq!(
        ids, expected,
        "the listing must report exactly the accepted jobs, got {ids:?}"
    );
}

/// One bucket name Amazon S3 holds no bucket under, with the published rule it breaks.
struct BadName {
    case: &'static str,
    name: String,
}

/// A name for each published general-purpose naming rule, plus one character past each
/// length bound. The reading of the rules is property-tested where it lives
/// (`services/common/src/s3_bucket_name.rs`); these rows pin what MediaPackage DOES with a
/// name outside them.
fn bad_names() -> Vec<BadName> {
    vec![
        BadName {
            case: "a name one character under the published floor",
            name: "d".repeat(MIN_BUCKET_NAME_CHARS - 1),
        },
        BadName {
            case: "a name one character over the published ceiling",
            name: format!("d{}", "x".repeat(MAX_BUCKET_NAME_CHARS)),
        },
        BadName {
            case: "an uppercase name",
            name: "Destination-Harvests".to_string(),
        },
        BadName {
            case: "a name carrying an underscore",
            name: "destination_harvests".to_string(),
        },
        BadName {
            case: "a name carrying a space",
            name: "destination harvests".to_string(),
        },
        BadName {
            case: "a name starting with a hyphen",
            name: "-destination-harvests".to_string(),
        },
        BadName {
            case: "a name ending with a dot",
            name: "destination-harvests.".to_string(),
        },
        BadName {
            case: "a name carrying two adjacent dots",
            name: "destination..harvests".to_string(),
        },
        BadName {
            case: "a name formatted as an IP address",
            name: "192.168.5.4".to_string(),
        },
        BadName {
            case: "a name carrying a reserved prefix",
            name: "xn--destination".to_string(),
        },
        BadName {
            case: "a name carrying a reserved suffix",
            name: "destination-s3alias".to_string(),
        },
        BadName {
            case: "a name that is not ASCII",
            name: "harvésts".to_string(),
        },
        BadName {
            case: "the empty name",
            name: String::new(),
        },
    ]
}

#[tokio::test]
async fn a_bucket_name_amazon_s3_holds_no_bucket_under_is_refused_and_persists_nothing() {
    let (router, _engine) = emulator();
    provision(&router).await;
    let role = create_role(&router, "mediapackage-destination-name", &assumable()).await;
    let requirement = state::harvest_destination::bucket_name_requirement();

    for (index, row) in bad_names().into_iter().enumerate() {
        let id = format!("bad-name-{index}");
        let answered = submit(
            &router,
            &submission(&id, ENDPOINT_ID, &row.name, "harvests/index.m3u8", &role),
        )
        .await;
        assert_refused(
            &router,
            row.case,
            &id,
            answered,
            BUCKET_MEMBER,
            &requirement,
        )
        .await;
    }
}

#[tokio::test]
async fn a_destination_bucket_the_account_does_not_hold_is_refused_and_persists_nothing() {
    let (router, _engine) = emulator();
    provision(&router).await;
    let role = create_role(&router, "mediapackage-destination-absent", &assumable()).await;
    let requirement = state::harvest_destination::writable_bucket_requirement();

    // Well-formed names Amazon S3 would hold a bucket under, that no `CreateBucket` ever
    // created. They differ in form, so the refusal is not a property of one spelling.
    let absent = [
        "destination-never-created",
        "destination.never.created",
        "neverthere3",
        &"n".repeat(MAX_BUCKET_NAME_CHARS),
    ];

    // One bucket IS created, so the rows below are refused for the bucket they name rather
    // than for an object store that holds nothing at all.
    create_bucket(&router, "destination-held").await;

    for (index, bucket) in absent.into_iter().enumerate() {
        let id = format!("absent-bucket-{index}");
        let case = format!("a submission naming the bucket {bucket:?}, which was never created");
        let answered = submit(
            &router,
            &submission(&id, ENDPOINT_ID, bucket, "harvests/index.m3u8", &role),
        )
        .await;
        assert_refused(&router, &case, &id, answered, BUCKET_MEMBER, &requirement).await;
    }

    // The control: the same submission against the bucket the account DOES hold is accepted,
    // so what the rows above were refused for is the bucket and nothing else.
    let (status, created) = submit(
        &router,
        &submission(
            "held-bucket",
            ENDPOINT_ID,
            "destination-held",
            "harvests/index.m3u8",
            &role,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the bucket the account holds\": expected 200, got {status} {created:?}"
    );
}

#[tokio::test]
async fn a_destination_role_the_service_could_not_act_as_is_refused_before_the_bucket() {
    let (router, _engine) = emulator();
    provision(&router).await;
    create_bucket(&router, "destination-role-harvests").await;
    let denied_role = create_role(
        &router,
        "mediapackage-destination-denied",
        &policy("Deny", "\"sts:AssumeRole\"", &format!("\"{PRINCIPAL}\"")),
    )
    .await;
    let role_requirement = state::assumable_role_requirement();

    // (case, the role named, the bucket named)
    let rows: Vec<(String, String, &str)> = vec![
        (
            "a role of this account that IAM holds nothing for".to_string(),
            format!("arn:aws:iam::{ACCOUNT}:role/never-created"),
            "destination-role-harvests",
        ),
        (
            "a role of this account under a path that IAM holds nothing for".to_string(),
            format!("arn:aws:iam::{ACCOUNT}:role/service-role/never-created"),
            "destination-role-harvests",
        ),
        (
            "an ARN carrying no account, which names no other account".to_string(),
            "arn:aws:iam:::role/never-created".to_string(),
            "destination-role-harvests",
        ),
        (
            "a role this account holds whose trust policy denies this service".to_string(),
            denied_role,
            "destination-role-harvests",
        ),
        (
            // The ORDER: a submission failing both is told to fix the role, because a
            // harvest the service could not write AS cannot be written anywhere.
            "a role IAM holds nothing for and a bucket the account does not hold".to_string(),
            format!("arn:aws:iam::{ACCOUNT}:role/never-created"),
            "destination-never-created",
        ),
    ];

    for (index, (case, role, bucket)) in rows.into_iter().enumerate() {
        let id = format!("bad-role-{index}");
        let answered = submit(
            &router,
            &submission(&id, ENDPOINT_ID, bucket, "harvests/index.m3u8", &role),
        )
        .await;
        assert_refused(
            &router,
            &case,
            &id,
            answered,
            ROLE_MEMBER,
            &role_requirement,
        )
        .await;
    }
}

#[tokio::test]
async fn the_endpoint_precondition_is_reported_before_the_destination() {
    let (router, _engine) = emulator();
    provision(&router).await;
    let role = create_role(&router, "mediapackage-destination-order", &assumable()).await;
    let absent_role = format!("arn:aws:iam::{ACCOUNT}:role/never-created");

    // Every row fails a precondition decided against the STORED endpoint the request acts on
    // AND the destination; the refusal names the endpoint member, because the destination is
    // not what such a caller has to fix. The rows span both endpoint failures (a reference
    // that resolves to nothing, a resolved endpoint retaining no startover window) against
    // both destination failures (the bucket, the role). The last row is the exception that
    // shows where the line is: a malformed bucket NAME is a value in the request, judged with
    // the request's other members before any stored resource is resolved, so IT is reported.
    //
    // (case, the endpoint named, the bucket named, the role named, the member reported)
    let rows: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "no such endpoint and a bucket the account does not hold",
            "destination-endpoint-never-created",
            "destination-never-created",
            &role,
            "originEndpointId",
        ),
        (
            "no such endpoint and a role IAM holds nothing for",
            "destination-endpoint-never-created",
            "destination-never-created",
            &absent_role,
            "originEndpointId",
        ),
        (
            "an endpoint retaining no startover window and a bucket the account does not hold",
            CLOSED_ENDPOINT_ID,
            "destination-never-created",
            &role,
            "originEndpointId",
        ),
        (
            "an endpoint retaining no startover window and a role IAM holds nothing for",
            CLOSED_ENDPOINT_ID,
            "destination-never-created",
            &absent_role,
            "originEndpointId",
        ),
        (
            "an endpoint retaining no startover window and a malformed bucket name",
            CLOSED_ENDPOINT_ID,
            "Destination_Never_Created",
            &role,
            BUCKET_MEMBER,
        ),
    ];

    for (index, (case, endpoint, bucket, named_role, member)) in rows.into_iter().enumerate() {
        let id = format!("order-{index}");
        let (status, body) = submit(
            &router,
            &submission(&id, endpoint, bucket, "harvests/index.m3u8", named_role),
        )
        .await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status} {body:?}"
        );
        let keys: Vec<String> = body
            .get("errors")
            .and_then(Value::as_object)
            .map(|errors| errors.keys().cloned().collect())
            .unwrap_or_default();
        assert_eq!(
            keys,
            vec![member.to_string()],
            "case {case:?}: expected the refusal keyed by {member:?} alone, got {body:?}"
        );
        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused submission must persist no job, got {status} {described:?}"
        );
    }
}
