// SPDX-License-Identifier: CC0-1.0

//! The documented end-to-end flow a client drives the connector data plane through, as
//! that client sees it: by POLLING the API and nothing else.
//!
//! RULE under test: every stage of that flow reaches an observable TERMINAL state on the
//! clock, so a caller that only reads what the API serves — `DescribeServer.State` after
//! a `StopServer`/`StartServer`, `TestConnection`'s `SftpConnectionDetails.HostKey` and
//! `Status`, and `ListFileTransferResults[].StatusCode` after `StartFileTransfer` — sees
//! `OFFLINE`, then `ONLINE`, then the endpoint's host key, then `OK` once that key is
//! pinned, then a terminal per-file `StatusCode` naming the `FilePath` it sent, with the
//! bytes at the destination. Nothing in the chain stays non-terminal, and no stage needs
//! the caller to know how long the emulator takes over it.
//!
//! The suites beside this one assert each stage's own rule on its own table
//! ([`crate::connector_gated_workflow`] for the verdict of a connection and for what one
//! accepted transfer does to each file, [`crate::server_transition_windows`] for the
//! transition windows); what this one pins is that the WHOLE chain settles, which is
//! what a client following the documented sequence depends on. Its rows differ along the
//! shape of the transfer the settled connector then carries out: how many files one
//! transfer names (one, several, and the maximum the model allows) and whether the
//! destination is the directory the request names or the SFTP user's home directory.
//!
//! Secrets Manager and Amazon S3 are mounted beside the transfer service by the shared
//! [`Target`], because the connector data plane calls them for the credentials a session
//! authenticates with and for the bytes a transfer moves.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::connector_gated_workflow::{results, s3, Target, COMPLETED, FAILED, STATUS_OK};
use crate::harness::member_of;

/// How often this suite re-reads a stage it is waiting on — a client's poll, kept short
/// so the wait is bounded by the stage and not by the interval.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long a stage of the flow may take before this suite calls it stalled.
///
/// Deliberately NOT read from the spans in `src/state.rs`: what the rule says is that a
/// client who knows none of them still sees every stage settle, so the bound here is a
/// client's patience — far longer than any span this emulator chooses, and short enough
/// that a stage which never settles fails the test instead of hanging it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// `State`'s two terminal values this flow waits on (`smithy_query(query="shape",
/// shape="State")` -> `[OFFLINE, ONLINE, STARTING, STOPPING, START_FAILED, STOP_FAILED]`).
const OFFLINE: &str = "OFFLINE";
const ONLINE: &str = "ONLINE";

/// One row of the transfer table: how many files the transfer names and where they land.
struct Flow {
    case: &'static str,
    /// How many files this row's `SendFilePaths` names.
    files: usize,
    /// `RemoteDirectoryPath`, relative to the bucket — or `None` to send none, which
    /// lands the files in the SFTP user's home directory.
    remote_directory: Option<&'static str>,
}

/// The rows: one file, several files, and the MAXIMUM one transfer may name
/// (`SendFilePaths` targets `FilePaths`, `len 1..10`, from
/// `smithy_query(query="shape",shape="FilePaths")`), the last of them with no
/// `RemoteDirectoryPath` at all.
const FLOWS: &[Flow] = &[
    Flow {
        case: "one file into the remote directory the request names",
        files: 1,
        remote_directory: Some("landing"),
    },
    Flow {
        case: "three files into one remote directory",
        files: 3,
        remote_directory: Some("landing/batch"),
    },
    Flow {
        case: "the ten files FilePaths' length bound allows, into the user's home \
               directory",
        files: 10,
        remote_directory: None,
    },
];

/// RULE: the documented flow settles at every stage — the server the connector connects
/// to reaches `ONLINE` after the start that was asked of it, a probe of the connector
/// reports the endpoint's own host key and answers `OK` once that key is pinned, and each
/// accepted transfer's per-file result reaches a terminal `StatusCode` naming the
/// `FilePath` the request sent, with that file's bytes at the destination.
#[tokio::test]
async fn the_documented_end_to_end_flow_settles_at_every_stage() {
    let target = Target::with_home_at_bucket_root().await;
    let secret = target
        .store_secret(
            "aws/transfer/end-to-end",
            &target.user_name,
            &target.user_key.private,
        )
        .await;

    // Stage 1: the server the flow transfers onto is stopped and started again, and the
    // client learns it is serving from `DescribeServer` alone.
    let case = "a server stopped and started again";
    for (operation, terminal) in [("StopServer", OFFLINE), ("StartServer", ONLINE)] {
        let answer = target
            .emulator
            .call(operation, json!({"ServerId": &target.server_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected {operation} 200, got {} with body {}",
            answer.status, answer.body
        );
        let state = poll_until(case, &format!("DescribeServer reaches {terminal}"), || async {
            let described = target
                .emulator
                .call("DescribeServer", json!({"ServerId": &target.server_id}))
                .await;
            let state = member_of(described.get("Server"), "State").clone();
            (state == json!(terminal), state)
        })
        .await;
        assert_eq!(
            state,
            json!(terminal),
            "case {case:?}: expected the server to reach {terminal} after {operation}, \
             got {state:?}"
        );
    }

    // Stage 2: the connector is created trusting no host key, so the client reads the one
    // the endpoint presents out of its first probe — the key it then pins.
    let connector_id = target
        .create_connector(case, json!({"UserSecretId": &secret}), None)
        .await;
    let probed = poll_until(case, "TestConnection reports a host key", || async {
        let probe = target.test_connection(&connector_id).await;
        let host_key = member_of(probe.get("SftpConnectionDetails"), "HostKey").clone();
        (host_key.is_string(), host_key)
    })
    .await;
    let host_key = probed
        .as_str()
        .expect("the polled host key is the string the probe reported")
        .to_string();
    assert_eq!(
        host_key, target.host_key,
        "case {case:?}: expected the probe to report the server's own host key, got \
         {host_key:?}"
    );

    // Stage 3: pinning that key is what brings the connector into service, and the client
    // sees that from `Status` alone.
    let pinned = target
        .emulator
        .call(
            "UpdateConnector",
            json!({
                "ConnectorId": &connector_id,
                "SftpConfig": {"TrustedHostKeys": [&host_key]},
            }),
        )
        .await;
    assert_eq!(
        pinned.status, 200,
        "case {case:?}: expected UpdateConnector 200, got {} with body {}",
        pinned.status, pinned.body
    );
    let status = poll_until(case, "TestConnection answers OK", || async {
        let probe = target.test_connection(&connector_id).await;
        let status = probe.get("Status").clone();
        (status == json!(STATUS_OK), status)
    })
    .await;
    assert_eq!(
        status,
        json!(STATUS_OK),
        "case {case:?}: expected the pinned connector's test to answer {STATUS_OK}, got \
         {status:?}"
    );

    // Stage 4: each row's transfer settles on a terminal per-file result, and the bytes
    // are where that result says they went.
    for (row, flow) in FLOWS.iter().enumerate() {
        let case = flow.case;
        // Every file carries its OWN bytes, so a landed file is compared with its source
        // rather than with any file of this transfer.
        let sources: Vec<(String, Vec<u8>)> = (0..flow.files)
            .map(|index| {
                (
                    format!("outbox/r{row}/f{index}.csv"),
                    format!("case,{row},{index}\n").into_bytes(),
                )
            })
            .collect();
        for (key, body) in &sources {
            put_object(&target, key, body.clone()).await;
        }

        let sent: Vec<String> = sources
            .iter()
            .map(|(key, _)| format!("/{}/{key}", target.bucket))
            .collect();
        let mut request = json!({"ConnectorId": &connector_id, "SendFilePaths": &sent});
        if let Some(directory) = flow.remote_directory {
            request["RemoteDirectoryPath"] = json!(format!("/{}/{directory}", target.bucket));
        }
        let accepted = target.emulator.call("StartFileTransfer", request).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected StartFileTransfer 200, got {} with body {}",
            accepted.status, accepted.body
        );
        let transfer_id = accepted
            .get("TransferId")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a TransferId, got {}", accepted.body)
            })
            .to_string();

        // What the flow polls: every file's result reaching a terminal status.
        let reported = poll_until(
            case,
            "every ListFileTransferResults entry reaches a terminal StatusCode",
            || async {
                let reported = results(&target, &connector_id, &transfer_id).await;
                let settled = reported.len() == flow.files
                    && reported.iter().all(|result| {
                        matches!(
                            member_of(result, "StatusCode").as_str(),
                            Some(COMPLETED) | Some(FAILED)
                        )
                    });
                (settled, Value::Array(reported))
            },
        )
        .await;
        let reported = reported
            .as_array()
            .expect("the polled results are the list the read reported")
            .clone();

        for (index, ((key, body), result)) in sources.iter().zip(&reported).enumerate() {
            assert_eq!(
                member_of(result, "StatusCode").as_str(),
                Some(COMPLETED),
                "case {case:?}: expected file {index} to reach {COMPLETED}, got {result}"
            );
            // "The filename and path to where the file was sent to or retrieved from":
            // the path this request sent, at the position it sent it in.
            assert_eq!(
                member_of(result, "FilePath").as_str(),
                Some(sent[index].as_str()),
                "case {case:?}: expected file {index} to report the path it was sent as, \
                 got {result}"
            );
            let file_name = key
                .rsplit('/')
                .next()
                .expect("the source key names a file");
            let destination = match flow.remote_directory {
                Some(directory) => format!("{directory}/{file_name}"),
                None => file_name.to_string(),
            };
            let landed = target.object(&destination).await;
            assert_eq!(
                landed.as_deref(),
                Some(body.as_slice()),
                "case {case:?}: expected file {index} at {destination:?} to hold its own \
                 bytes, got {landed:?}"
            );
        }
    }
}

/// Re-read one stage until `probe` reports it settled, and answer what it last saw.
///
/// `probe` answers `(settled, what it read)`, so a stage that never settles fails with
/// the value the API was still serving — which is the evidence a stall leaves.
async fn poll_until<F, Fut>(case: &str, stage: &str, probe: F) -> Value
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = (bool, Value)>,
{
    let started = Instant::now();
    loop {
        let (settled, last) = probe().await;
        if settled {
            return last;
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "case {case:?}: {stage} did not settle within {SETTLE_DEADLINE:?}; the last \
             answer was {last:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Put one object into the target's bucket, with bytes of this test's own making.
///
/// [`Target::put_object`] takes `&'static [u8]`, which a per-file body computed here is
/// not, so this suite writes its own bodies through the same in-process S3 client.
async fn put_object(target: &Target, key: &str, body: Vec<u8>) {
    s3(&target.emulator)
        .put_object()
        .bucket(&target.bucket)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await
        .expect("the emulated S3 stores the source object");
}
