// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of what the account input policy DOES: how a policy stored by
//! `PutPolicy` gates a job submitted afterwards.
//!
//! The rule every case below is one row of: a job submission is refused when ANY input the
//! job will read is at an input location the account's stored policy marks `DISALLOWED`,
//! and accepted when every one of them is at a location the policy leaves allowed — a
//! location the policy does not state carries no decision and is allowed, which is also
//! what an account with no policy at all allows. An input's LOCATION is what the scheme of
//! its `Settings.Inputs[].FileInput` names: `s3://` is `S3Inputs`, `http://` is
//! `HttpInputs`, `https://` is `HttpsInputs`.
//!
//! The members and their documented purpose come from the model:
//! `smithy_query(query="shape",shape="Policy",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`
//! — each member is "Allow or disallow jobs that specify <location> inputs" — and
//! `smithy_query(query="shape",shape="InputPolicy")`: "An input policy allows or disallows
//! a job you submit to run based on the conditions that you specify". The two forms an
//! input can take come from `Input$FileInput`, whose pattern admits `s3://…` and
//! `https?://…` only.
//!
//! A refused submission stores NOTHING, and it is refused with the modelled
//! `ForbiddenException` (403): the request is well formed and every resource it names
//! exists, and what refuses it is a permission the account itself withheld.
//!
//! The wire harness is the one `job_lifecycle` builds, so every case asserts on the bytes
//! a client would see.

use serde_json::{json, Map, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::ops::types::INPUT_POLICY_VALUES;

use super::job_lifecycle::{settings as job_settings, Emulator, Reply, ROLE};
use super::job_template_crud::settings as template_settings;

/// The policy's URI — the `http` binding the policy operations share
/// (`smithy_query(query="operation",operation="GetPolicy,PutPolicy,DeletePolicy")`).
const POLICY_URI: &str = "/2017-08-29/policy";

/// One input location, in the three spellings a case needs: the `jsonName` its `Policy`
/// member goes on the wire with, the Smithy member name a refusal names, and the URI
/// scheme an input at that location carries.
struct Location {
    wire: &'static str,
    member: &'static str,
    scheme: &'static str,
}

/// Every input location the `Policy` shape models, in its member order
/// (`smithy_query(query="shape",shape="Policy")`).
const LOCATIONS: [Location; 3] = [
    Location {
        wire: "s3Inputs",
        member: "S3Inputs",
        scheme: "s3",
    },
    Location {
        wire: "httpInputs",
        member: "HttpInputs",
        scheme: "http",
    },
    Location {
        wire: "httpsInputs",
        member: "HttpsInputs",
        scheme: "https",
    },
];

/// The location an input URI is at, or `None` when its scheme names none of the locations
/// the policy models — a value no policy decides about.
fn location_of(uri: &str) -> Option<&'static Location> {
    let (scheme, _) = uri.split_once("://")?;
    LOCATIONS.iter().find(|location| location.scheme == scheme)
}

/// THE RULE, as the cases below check their expectations against: the first of `uris` the
/// stated policy forbids a job from reading, with the `Policy` member whose decision
/// forbids it, or `None` when the policy allows every one of them.
fn first_disallowed<'a>(policy: &Value, uris: &[&'a str]) -> Option<(&'a str, &'static str)> {
    uris.iter().find_map(|uri| {
        let location = location_of(uri)?;
        (policy.get(location.wire).and_then(Value::as_str) == Some("DISALLOWED"))
            .then_some((*uri, location.member))
    })
}

/// Whether a stated policy leaves NO input location allowed — the one policy `PutPolicy`
/// itself refuses, so no job is ever submitted under it.
fn allows_no_input(policy: &Value) -> bool {
    LOCATIONS
        .iter()
        .all(|location| policy.get(location.wire).and_then(Value::as_str) == Some("DISALLOWED"))
}

/// Every policy a caller can state and `PutPolicy` will store: each of the three input
/// locations either carries one of the modelled `InputPolicy` values or is left out
/// entirely, minus the all-`DISALLOWED` policy AWS refuses.
///
/// Drawn from the generated `INPUT_POLICY_VALUES` — the model's own value set
/// (`smithy_query(query="shape",shape="InputPolicy")`) — rather than a hand-typed list, so
/// the table cannot drift from the model.
fn storable_policies() -> Vec<Value> {
    let choices: Vec<Option<&str>> = std::iter::once(None)
        .chain(INPUT_POLICY_VALUES.iter().copied().map(Some))
        .collect();
    let mut rows = Vec::new();
    for s3 in &choices {
        for http in &choices {
            for https in &choices {
                let mut policy = Map::new();
                for (location, choice) in LOCATIONS.iter().zip([s3, http, https]) {
                    if let Some(value) = choice {
                        policy.insert(location.wire.to_string(), Value::String((*value).to_string()));
                    }
                }
                let policy = Value::Object(policy);
                if !allows_no_input(&policy) {
                    rows.push(policy);
                }
            }
        }
    }
    rows
}

/// A `JobSettings` whose inputs are exactly `uris`, with the output groups the shared job
/// fixture builds.
///
/// Those output groups write to `s3://`, in every row: the policy decides about a job's
/// INPUTS, so an output destination must never be what refuses a submission. An empty
/// `uris` builds the one input that states no file at all — a generated video — which is
/// therefore at no input location.
fn settings_for(uris: &[&str]) -> Value {
    let inputs: Vec<Value> = if uris.is_empty() {
        vec![json!({ "videoGenerator": { "duration": 5_000 } })]
    } else {
        uris.iter().map(|uri| json!({ "fileInput": uri })).collect()
    };
    json!({
        "inputs": inputs,
        "outputGroups": job_settings("s3://example-bucket/in/ignored.mp4", "_a")["outputGroups"]
            .clone()
    })
}

async fn put_policy(emulator: &Emulator, policy: &Value) -> Reply {
    emulator
        .send("PUT", POLICY_URI, Some(json!({ "policy": policy })))
        .await
}

async fn delete_policy(emulator: &Emulator) -> Reply {
    emulator.send("DELETE", POLICY_URI, None).await
}

/// Submit a job whose inputs are `uris`, with no job template.
async fn create_job_with_inputs(emulator: &Emulator, uris: &[&str]) -> Reply {
    emulator
        .create_job(json!({ "role": ROLE, "settings": settings_for(uris) }))
        .await
}

/// How many jobs the store holds, read back through `ListJobs` — through the API, never by
/// reaching into the state struct. Every case keeps its job count under one page.
async fn stored_jobs(emulator: &Emulator) -> usize {
    let listed = emulator.send("GET", "/2017-08-29/jobs", None).await;
    assert_eq!(
        listed.status, 200,
        "ListJobs must answer HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed.at("/jobs").as_array().map_or(usize::MAX, Vec::len)
}

/// Assert a submission was refused exactly as the rule requires: `ForbiddenException`
/// (403), naming the policy member that refused it and the input it refused.
fn assert_policy_refusal(case: &str, reply: &Reply, uri: &str, member: &str) {
    assert_eq!(
        reply.status, 403,
        "case {case}: expected HTTP 403, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ForbiddenException"),
        "case {case}: expected ForbiddenException, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    // The message is built from the request and the policy member that refused it, so the
    // rule is that it names both; no recorded prose is asserted.
    let message = reply.string("/message");
    for named in [uri, member, "DISALLOWED"] {
        assert!(
            message.contains(named),
            "case {case}: the refusal must name {named:?}, got message {message:?}"
        );
    }
    assert!(
        reply.at("/job").is_null(),
        "case {case}: a refused submission must answer no job, got {}",
        reply.body
    );
}

/// One row of the input table: the inputs a submitted job states.
struct Inputs {
    case: &'static str,
    uris: Vec<&'static str>,
}

/// The input rows every policy is driven against. They differ along what the rule
/// generalises over: which location each input is at, how many inputs the job states, the
/// ORDER a disallowed input arrives in, and an input at no location at all.
fn input_rows() -> Vec<Inputs> {
    vec![
        Inputs {
            case: "one Amazon S3 input",
            uris: vec!["s3://example-bucket/in/a.mp4"],
        },
        Inputs {
            case: "one HTTP input",
            uris: vec!["http://example.com/in.mp4"],
        },
        Inputs {
            case: "one HTTPS input",
            uris: vec!["https://example.com/in.mp4"],
        },
        Inputs {
            case: "an S3 input followed by an HTTP one",
            uris: vec![
                "s3://example-bucket/in/a.mp4",
                "http://example.com/second.mp4",
            ],
        },
        Inputs {
            case: "an HTTPS input followed by an S3 one",
            uris: vec![
                "https://example.com/first.mp4",
                "s3://example-bucket/in/b.mov",
            ],
        },
        Inputs {
            case: "one input at each of the three locations",
            uris: vec![
                "s3://example-bucket/in/a.mp4",
                "http://example.com/b.mp4",
                "https://example.com/c.mp4",
            ],
        },
        Inputs {
            case: "a generated-video input, which states no file at all",
            uris: vec![],
        },
    ]
}

/// A job is refused exactly when one of its inputs is at a location the account's policy
/// disallows, and stored otherwise.
///
/// Every storable policy — all 26 of them — is driven against every input row, and each
/// row's expectation is computed from the rule ([`first_disallowed`]) rather than listed by
/// hand. A refused submission must leave the store exactly as it was, which each policy's
/// own `ListJobs` count checks.
#[tokio::test]
async fn a_job_is_refused_when_its_input_is_at_a_location_the_policy_disallows() {
    let policies = storable_policies();
    assert_eq!(
        policies.len(),
        26,
        "the table must span every storable choice of every input location, got {} rows from \
         {INPUT_POLICY_VALUES:?}",
        policies.len()
    );

    for policy in &policies {
        // One account per policy, so the job count a refusal is checked against is this
        // policy's own and stays well inside one `ListJobs` page.
        let emulator = Emulator::new();
        let put = put_policy(&emulator, policy).await;
        assert_eq!(
            put.status, 200,
            "case {policy}: the policy must be stored, got {} with body {}",
            put.status, put.body
        );

        let mut accepted = 0;
        for row in input_rows() {
            let Inputs { case, uris } = row;
            let case = format!("{policy} / {case}");
            let reply = create_job_with_inputs(&emulator, &uris).await;
            match first_disallowed(policy, &uris) {
                Some((uri, member)) => assert_policy_refusal(&case, &reply, uri, member),
                None => {
                    assert_eq!(
                        reply.status, 201,
                        "case {case}: every input is at an allowed location, so the job must \
                         be created: got {} with body {}",
                        reply.status, reply.body
                    );
                    accepted += 1;
                    // Round trip: the accepted job reads back with the inputs it stated.
                    let id = reply.string("/job/id");
                    let fetched = emulator.get_job(&id).await;
                    assert_eq!(
                        fetched.status, 200,
                        "case {case}: expected GetJob HTTP 200, got {} with body {}",
                        fetched.status, fetched.body
                    );
                    assert_eq!(
                        fetched.at("/job"),
                        reply.at("/job"),
                        "case {case}: GetJob must return what CreateJob persisted, got {}",
                        fetched.body
                    );
                    let stored_uris: Vec<String> = fetched
                        .at("/job/settings/inputs")
                        .as_array()
                        .map(|inputs| {
                            inputs
                                .iter()
                                .filter_map(|input| input.get("fileInput"))
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    assert_eq!(
                        stored_uris, uris,
                        "case {case}: the stored job must report the inputs it was submitted \
                         with, got {stored_uris:?}"
                    );
                }
            }
        }

        let stored = stored_jobs(&emulator).await;
        assert_eq!(
            stored, accepted,
            "case {policy}: only the accepted submissions may be stored: expected {accepted}, \
             got {stored}"
        );
    }
}

/// The policy IN FORCE when a job is submitted is the one that governs it: putting a
/// policy starts refusing the submissions it disallows, deleting it stops, and neither
/// touches a job that was already stored.
#[tokio::test]
async fn the_policy_in_force_at_submit_time_governs_the_submission() {
    let emulator = Emulator::new();
    let s3 = vec!["s3://example-bucket/in/a.mp4"];
    let http = vec!["http://example.com/in.mp4"];

    // With no policy at all, every location is allowed.
    let first = create_job_with_inputs(&emulator, &http).await;
    assert_eq!(
        first.status, 201,
        "case no policy stored: an account with no policy allows every input location, got {} \
         with body {}",
        first.status, first.body
    );
    let kept = first.string("/job/id");

    // The steps a single account is walked through: the policy it carries, and how many
    // jobs it is expected to hold once the two submissions below have been answered.
    let steps: Vec<(&str, Option<Value>)> = vec![
        (
            "HTTP disallowed",
            Some(json!({ "s3Inputs": "ALLOWED", "httpInputs": "DISALLOWED" })),
        ),
        ("the policy deleted", None),
        (
            "S3 disallowed instead",
            Some(json!({ "s3Inputs": "DISALLOWED" })),
        ),
        (
            "every location allowed explicitly",
            Some(json!({
                "s3Inputs": "ALLOWED",
                "httpInputs": "ALLOWED",
                "httpsInputs": "ALLOWED"
            })),
        ),
    ];

    let mut expected_jobs = 1;
    for (case, policy) in &steps {
        match policy {
            Some(policy) => {
                let put = put_policy(&emulator, policy).await;
                assert_eq!(
                    put.status, 200,
                    "case {case}: the policy must be stored, got {} with body {}",
                    put.status, put.body
                );
            }
            None => {
                let deleted = delete_policy(&emulator).await;
                assert_eq!(
                    deleted.status, 200,
                    "case {case}: DeletePolicy must answer HTTP 200, got {} with body {}",
                    deleted.status, deleted.body
                );
            }
        }
        let in_force = policy.clone().unwrap_or_else(|| json!({}));

        for uris in [&s3, &http] {
            let reply = create_job_with_inputs(&emulator, uris).await;
            let case = format!("{case} / {uris:?}");
            match first_disallowed(&in_force, uris) {
                Some((uri, member)) => assert_policy_refusal(&case, &reply, uri, member),
                None => {
                    assert_eq!(
                        reply.status, 201,
                        "case {case}: the policy in force allows these inputs, so the job must \
                         be created: got {} with body {}",
                        reply.status, reply.body
                    );
                    expected_jobs += 1;
                }
            }
        }

        // A policy decides about SUBMISSIONS: the job stored before any policy existed is
        // still there and still readable, whatever the account allows now.
        let fetched = emulator.get_job(&kept).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: a job stored earlier must still read back, got {} with body {}",
            fetched.status, fetched.body
        );
        let stored = stored_jobs(&emulator).await;
        assert_eq!(
            stored, expected_jobs,
            "case {case}: expected {expected_jobs} stored jobs, got {stored}"
        );
    }
}

/// The policy is applied to the inputs the job will ACTUALLY read — the request's own
/// `Settings` laid over the job template it was created from — so a job created from a
/// template is gated exactly as one that spells its settings out.
///
/// A job template's `Inputs` are `InputTemplate`s, which carry no file input
/// (`smithy_query(query="shape",shape="InputTemplate")`), so the files a templated job
/// reads are the ones its own request states; the template here supplies the output groups.
#[tokio::test]
async fn a_job_created_from_a_template_is_gated_by_the_same_policy() {
    let emulator = Emulator::new();
    let template = emulator
        .create_job_template(json!({
            "name": "outputs-from-here",
            "settings": template_settings(1, 1)
        }))
        .await;
    assert_eq!(
        template.status, 201,
        "the fixture job template must be created: got {} with body {}",
        template.status, template.body
    );

    let policy = json!({ "s3Inputs": "ALLOWED", "httpInputs": "DISALLOWED" });
    let put = put_policy(&emulator, &policy).await;
    assert_eq!(
        put.status, 200,
        "the policy must be stored, got {} with body {}",
        put.status, put.body
    );

    let rows = [
        vec!["http://example.com/in.mp4"],
        vec!["s3://example-bucket/in/a.mp4"],
        vec![
            "s3://example-bucket/in/a.mp4",
            "http://example.com/second.mp4",
        ],
    ];

    let mut accepted = 0;
    for uris in &rows {
        let case = format!("templated job with inputs {uris:?}");
        let inputs: Vec<Value> = uris.iter().map(|uri| json!({ "fileInput": uri })).collect();
        let reply = emulator
            .create_job(json!({
                "role": ROLE,
                "jobTemplate": "outputs-from-here",
                "settings": { "inputs": inputs }
            }))
            .await;
        match first_disallowed(&policy, uris) {
            Some((uri, member)) => assert_policy_refusal(&case, &reply, uri, member),
            None => {
                assert_eq!(
                    reply.status, 201,
                    "case {case}: every input is at an allowed location, so the job must be \
                     created: got {} with body {}",
                    reply.status, reply.body
                );
                accepted += 1;
            }
        }
    }

    let stored = stored_jobs(&emulator).await;
    assert_eq!(
        stored, accepted,
        "only the accepted submissions may be stored: expected {accepted}, got {stored}"
    );
}
