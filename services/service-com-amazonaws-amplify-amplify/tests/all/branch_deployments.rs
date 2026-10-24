// SPDX-License-Identifier: CC0-1.0

//! `CreateDeployment`: reserving a manual deployment on a branch of an Amplify app.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. A reservation answers the upload destinations for what the request asked to
//!    upload: a request that NAMES files gets one destination per file name, keyed by
//!    that name, and no single zip destination; a request that names none gets the one
//!    destination for the zipped deployment. Every destination is unique and inside
//!    `UploadUrl`'s modelled length bound, whatever the request named its files.
//! 2. A reservation takes a `jobId` out of its branch's id sequence — decimal digits,
//!    as `JobId` is modelled — and that id is SPENT: a later reservation and a later
//!    `StartJob` on the same branch are handed different ones.
//! 3. A reservation is not a job: nothing is readable through `GetJob` under the
//!    reserved id, and the branch has still started no job, until a start call runs it.
//! 4. A deployment is reserved on a branch of an app, so an app id or a branch name
//!    that names nothing is refused with this operation's modelled 400 rather than
//!    created on the way.
//! 5. One branch's unstarted reservations are BOUNDED: past the cap, one more is
//!    refused with the modelled `LimitExceededException`.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives;
//! only the cases are new.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::MAX_PENDING_DEPLOYMENTS_PER_BRANCH;

use crate::app_branch_lifecycle::{string_member, Emulator, Reply};

/// `UploadUrl`'s modelled maximum length, which every destination this operation
/// returns has to fit in (`smithy_query(query="shape",shape="UploadUrl")`:
/// `length 0..1000`).
const UPLOAD_URL_MAX: usize = 1000;

/// `FileName`'s modelled maximum length
/// (`smithy_query(query="shape",shape="FileMap")`: key `length 0..255`), so a request
/// may name a file 255 characters long.
const FILE_NAME_MAX: usize = 255;

/// `CreateDeployment` over the wire.
async fn create_deployment(
    emulator: &Emulator,
    app_id: &str,
    branch: &str,
    request: &Value,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch}/deployments"),
            Some(request),
        )
        .await
}

/// `StartJob` over the wire.
async fn start_job(emulator: &Emulator, app_id: &str, branch: &str, request: &Value) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch}/jobs"),
            Some(request),
        )
        .await
}

/// `GetJob` over the wire.
async fn get_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}"),
            None,
        )
        .await
}

/// An app with one branch on it.
async fn app_with_branch(emulator: &Emulator, name: &str, branch: &str) -> String {
    let app_id = emulator.app_id_of_new_app(name).await;
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
    );
    app_id
}

/// The `fileUploadUrls` of a reply, as the map a client reads: the empty map when the
/// body carries no destination per file.
fn file_upload_urls(reply: &Reply) -> BTreeMap<String, String> {
    reply
        .body
        .get("fileUploadUrls")
        .and_then(Value::as_object)
        .map(|members| {
            members
                .iter()
                .map(|(name, url)| {
                    (
                        name.clone(),
                        url.as_str()
                            .unwrap_or_else(|| panic!("`{name}` is not a string in {reply:?}"))
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `zipUploadUrl` of a reply, as the value a client reads: the empty string when
/// the body carries no single zip destination.
fn zip_upload_url(reply: &Reply) -> String {
    reply
        .body
        .get("zipUploadUrl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Does `candidate` satisfy `JobId`'s modelled `pattern ^[0-9]+$` and `length 0..255`
/// (`smithy_query(query="shape",shape="JobId")`)?
fn is_modelled_job_id(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= 255
        && candidate.chars().all(|digit| digit.is_ascii_digit())
}

/// One `CreateDeployment` request, and the file names the rule says its answer must
/// carry a destination for.
struct DeploymentCase {
    case: String,
    request: Value,
    expected_files: Vec<String>,
}

/// The table: one row per way a request can describe what it is about to upload.
fn deployment_cases() -> Vec<DeploymentCase> {
    // `MD5Hash` is modelled `length 0..32` with no further constraint
    // (`smithy_query(query="shape",shape="FileMap")`), so the empty hash is a value a
    // request may legitimately declare and the rule does not read any of them.
    let hash = "d41d8cd98f00b204e9800998ecf8427e";
    let longest_name = "f".repeat(FILE_NAME_MAX);
    let mut longest = serde_json::Map::new();
    longest.insert(longest_name.clone(), Value::String(hash.to_string()));
    vec![
        DeploymentCase {
            case: "no fileMap member".to_string(),
            request: json!({}),
            expected_files: Vec::new(),
        },
        DeploymentCase {
            case: "fileMap naming no file".to_string(),
            request: json!({ "fileMap": {} }),
            expected_files: Vec::new(),
        },
        DeploymentCase {
            case: "one file".to_string(),
            request: json!({ "fileMap": { "index.html": hash } }),
            expected_files: vec!["index.html".to_string()],
        },
        DeploymentCase {
            case: "several files under nested prefixes".to_string(),
            request: json!({
                "fileMap": {
                    "index.html": hash,
                    "assets/app.js": hash,
                    "assets/styles/main.css": hash,
                }
            }),
            expected_files: vec![
                "assets/app.js".to_string(),
                "assets/styles/main.css".to_string(),
                "index.html".to_string(),
            ],
        },
        DeploymentCase {
            case: "file names carrying spaces, query syntax and non-ASCII".to_string(),
            request: json!({
                "fileMap": {
                    "a file with spaces.txt": hash,
                    "weird?name=1&x#y.txt": hash,
                    "ünïcode-ƒile.txt": hash,
                }
            }),
            expected_files: vec![
                "a file with spaces.txt".to_string(),
                "weird?name=1&x#y.txt".to_string(),
                "ünïcode-ƒile.txt".to_string(),
            ],
        },
        DeploymentCase {
            case: "a file whose declared hash is empty".to_string(),
            request: json!({ "fileMap": { "empty.txt": "" } }),
            expected_files: vec!["empty.txt".to_string()],
        },
        DeploymentCase {
            case: "the longest modelled file name".to_string(),
            request: json!({ "fileMap": Value::Object(longest) }),
            expected_files: vec![longest_name],
        },
    ]
}

/// Rule 1: the destinations a reservation hands back are the ones the request asked
/// for — one per named file, or the single zip destination when it named none.
#[tokio::test]
async fn reserves_one_upload_destination_per_thing_to_upload() {
    let emulator = Emulator::new();
    let app_id = app_with_branch(&emulator, "deployment-destinations", "main").await;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for DeploymentCase {
        case,
        request,
        expected_files,
    } in deployment_cases()
    {
        let reply = create_deployment(&emulator, &app_id, "main", &request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDeployment, got {reply:?}"
        );

        let job_id = string_member(&reply.body, "jobId");
        assert!(
            is_modelled_job_id(&job_id),
            "case {case:?}: expected a modelled jobId of decimal digits, got {job_id:?}"
        );

        let urls = file_upload_urls(&reply);
        assert_eq!(
            urls.keys().cloned().collect::<Vec<String>>(),
            expected_files,
            "case {case:?}: expected a destination for exactly the files named, got {urls:?}"
        );

        let zip = zip_upload_url(&reply);
        if expected_files.is_empty() {
            assert!(
                !zip.is_empty(),
                "case {case:?}: a request naming no file uploads one zip, got {zip:?}"
            );
        } else {
            assert!(
                zip.is_empty(),
                "case {case:?}: a request naming its files uploads no zip, got {zip:?}"
            );
        }

        // Every destination of every reservation: well formed, inside the modelled
        // length bound, naming the deployment it uploads to, and never repeated.
        for url in urls.values().chain(if zip.is_empty() { None } else { Some(&zip) }) {
            assert!(
                url.starts_with("https://") && url.len() <= UPLOAD_URL_MAX,
                "case {case:?}: expected an https destination of at most \
                 {UPLOAD_URL_MAX} characters, got {} characters: {url:?}",
                url.len()
            );
            assert!(
                url.contains(&app_id) && url.contains(&format!("/{job_id}/")),
                "case {case:?}: a destination names the deployment it uploads to, got {url:?}"
            );
            assert!(
                seen.insert(url.clone()),
                "case {case:?}: every upload has its OWN destination, got the repeated {url:?}"
            );
        }
    }
}

/// Rules 2 and 3: a reserved `jobId` is spent and is not a job.
#[tokio::test]
async fn a_reserved_job_id_is_spent_and_is_not_a_job() {
    let emulator = Emulator::new();
    let app_id = app_with_branch(&emulator, "deployment-ids", "main").await;

    // Three reservations, differing in what each asked to upload.
    let mut reserved: Vec<String> = Vec::new();
    for request in [
        json!({}),
        json!({ "fileMap": { "index.html": "d41d8cd98f00b204e9800998ecf8427e" } }),
        json!({ "fileMap": {} }),
    ] {
        let reply = create_deployment(&emulator, &app_id, "main", &request).await;
        assert_eq!(
            reply.status, 200,
            "case {request:?}: expected HTTP 200 from CreateDeployment, got {reply:?}"
        );
        reserved.push(string_member(&reply.body, "jobId"));
    }

    assert_eq!(
        reserved.iter().collect::<BTreeSet<&String>>().len(),
        reserved.len(),
        "case \"three reservations\": each reservation spends its own jobId, got {reserved:?}"
    );
    let numeric: Vec<u64> = reserved
        .iter()
        .map(|job_id| job_id.parse::<u64>().expect("a modelled jobId parses"))
        .collect();
    assert!(
        numeric.windows(2).all(|pair| pair[0] < pair[1]),
        "case \"three reservations\": ids come from the branch's sequence, got {reserved:?}"
    );

    // A reservation is not a job: nothing answers under the reserved id, and the
    // branch has still started nothing.
    for job_id in &reserved {
        let reply = get_job(&emulator, &app_id, "main", job_id).await;
        assert_eq!(
            reply.status, 404,
            "case \"reserved {job_id}\": a reservation starts no job, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("NotFoundException"),
            "case \"reserved {job_id}\": expected the modelled 404, got {:?}",
            reply.error_code
        );
    }
    let record = emulator.get_branch(&app_id, "main").await.member("branch");
    assert_eq!(
        string_member(&record, "totalNumberOfJobs"),
        "0",
        "case \"reservations only\": a reserved deployment is not a started job, got {record:?}"
    );

    // The next job this branch starts is handed a DIFFERENT id, and only that started
    // job is readable.
    let started = start_job(&emulator, &app_id, "main", &json!({ "jobType": "MANUAL" })).await;
    assert_eq!(
        started.status, 200,
        "case \"StartJob after reservations\": expected HTTP 200, got {started:?}"
    );
    let started_id = string_member(&started.member("jobSummary"), "jobId");
    assert!(
        !reserved.contains(&started_id),
        "case \"StartJob after reservations\": a reserved id is spent, got {started_id:?} \
         among {reserved:?}"
    );
    assert!(
        started_id.parse::<u64>().expect("a modelled jobId parses")
            > *numeric.last().expect("three reservations"),
        "case \"StartJob after reservations\": the id sequence only moves forward, \
         got {started_id:?} after {reserved:?}"
    );
    assert_eq!(
        get_job(&emulator, &app_id, "main", &started_id).await.status,
        200,
        "case \"StartJob after reservations\": the started job is readable"
    );
    let record = emulator.get_branch(&app_id, "main").await.member("branch");
    assert_eq!(
        string_member(&record, "totalNumberOfJobs"),
        "1",
        "case \"StartJob after reservations\": only started jobs are counted, got {record:?}"
    );

    // And a reservation taken AFTER a job still gets an unspent id.
    let later = create_deployment(&emulator, &app_id, "main", &json!({})).await;
    let later_id = string_member(&later.body, "jobId");
    assert!(
        later_id != started_id && !reserved.contains(&later_id),
        "case \"reservation after a job\": every id is handed out once, got {later_id:?}"
    );
}

/// Rule 4: a deployment is reserved on a branch of an app, so an identifier that
/// names nothing is refused with this operation's modelled 400.
#[tokio::test]
async fn refuses_an_app_or_branch_that_names_nothing() {
    let emulator = Emulator::new();
    let app_id = app_with_branch(&emulator, "deployment-refusals", "main").await;
    // A branch of ANOTHER app: a reservation is reached through its own app, so this
    // name exists in the store and still names nothing on this app.
    let elsewhere = app_with_branch(&emulator, "deployment-refusals-elsewhere", "staging").await;
    assert_ne!(
        app_id, elsewhere,
        "two apps must have distinct ids: got {app_id:?} twice"
    );

    // Each row: the app id and branch name the request addresses, and the entity plus
    // identifier the refusal has to name.
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "unknown app",
            "d0000000000000".to_string(),
            "main".to_string(),
            "App",
            "d0000000000000".to_string(),
        ),
        (
            "unknown branch",
            app_id.clone(),
            "no-such-branch".to_string(),
            "Branch",
            "no-such-branch".to_string(),
        ),
        (
            "branch of another app",
            app_id.clone(),
            "staging".to_string(),
            "Branch",
            "staging".to_string(),
        ),
    ];

    for (case, addressed_app, addressed_branch, entity, identifier) in cases {
        for request in [json!({}), json!({ "fileMap": { "index.html": "" } })] {
            let reply =
                create_deployment(&emulator, &addressed_app, &addressed_branch, &request).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected the modelled HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("BadRequestException"),
                "case {case:?}: expected the modelled BadRequestException, got {:?}",
                reply.error_code
            );
            let expected = format!("{entity} {identifier} not found.");
            assert_eq!(
                reply.message().as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the request-derived message {expected:?}, got {:?}",
                reply.message()
            );
        }
    }

    // A refused reservation spends nothing: the branch's next id is still the first.
    let reply = create_deployment(&emulator, &app_id, "main", &json!({})).await;
    assert_eq!(
        string_member(&reply.body, "jobId"),
        "1",
        "case \"refusals spend nothing\": a refused reservation leaves the store alone, \
         got {reply:?}"
    );
}

/// Rule 5: one branch's unstarted reservations are bounded.
#[tokio::test]
async fn unstarted_reservations_are_bounded() {
    let emulator = Emulator::new();
    let app_id = app_with_branch(&emulator, "deployment-cap", "main").await;

    for reservation in 1..=MAX_PENDING_DEPLOYMENTS_PER_BRANCH {
        let reply = create_deployment(&emulator, &app_id, "main", &json!({})).await;
        assert_eq!(
            reply.status, 200,
            "case \"reservation {reservation} of {MAX_PENDING_DEPLOYMENTS_PER_BRANCH}\": \
             expected HTTP 200, got {reply:?}"
        );
    }

    let refused = create_deployment(&emulator, &app_id, "main", &json!({})).await;
    assert_eq!(
        refused.status, 429,
        "case \"past the cap\": expected the modelled quota refusal, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("LimitExceededException"),
        "case \"past the cap\": expected LimitExceededException, got {:?}",
        refused.error_code
    );
    let message = refused.message().unwrap_or_default();
    assert!(
        message.contains(&MAX_PENDING_DEPLOYMENTS_PER_BRANCH.to_string()) && message.contains("main"),
        "case \"past the cap\": the refusal must name the limit and the branch, got {message:?}"
    );

    // The cap is PER BRANCH: another branch of the same app still reserves.
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": "other" }))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"another branch\": expected HTTP 200 from CreateBranch, got {created:?}"
    );
    let reply = create_deployment(&emulator, &app_id, "other", &json!({})).await;
    assert_eq!(
        reply.status, 200,
        "case \"another branch\": one branch's cap does not bind another, got {reply:?}"
    );
}
