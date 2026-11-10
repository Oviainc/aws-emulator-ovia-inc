// SPDX-License-Identifier: CC0-1.0

//! The IoT data-plane bootstrap sequence, across TWO services: a client asks the
//! IoT control plane where its data plane lives (`DescribeEndpoint`), then talks
//! to the IoT Jobs data plane at the address it was given.
//!
//! This suite needs the WHOLE router: the two halves of the sequence are served
//! by two different crates, so a single-service test cannot see that discovery
//! answers at all and that the data plane it points at is mounted and serving.
//!
//! What it does and does not prove. This router dispatches on the SigV4
//! credential scope (`emulator_server::resolve_route_key` reads the signing name
//! first), NOT on the `Host` header, so a jobs-data request would be served
//! whatever host it addressed; the second leg therefore proves the data plane is
//! mounted, reachable and stateful at the address discovery hands back, not that
//! the host is the dispatch key. The address itself is pinned here as the address
//! this emulator is SERVED at — the one a client can dial, for every endpoint type
//! and across repeated calls — and more thoroughly in
//! `services/service-com-amazonaws-iot-awsiotservice/tests/all/describe_endpoint.rs`.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::Value;
use smithy_server_runtime::EndpointUrl;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The address this emulator is served at, as the core publishes it from its data
/// port — the base every address a handler hands out names. A client reaches the
/// emulator here and nowhere else, which is why discovery has to report it.
const SERVED_AT: &str = "https://127.0.0.1:39355";

/// The `host[:port]` of [`SERVED_AT`]: what a published address is, since what a
/// client is handed is an address it appends a path to, not a URL.
const SERVED_AUTHORITY: &str = "127.0.0.1:39355";

/// A command for the data-plane leg to start an execution of. Its account
/// segment is arbitrary: the Jobs data plane reads the thing name out of the
/// target ARN and keeps the command ARN as given.
///
/// It is REGISTERED in the shared IoT command index below, because
/// `StartCommandExecution` starts an execution only for a command that exists.
/// The control plane's `CreateCommand` is the writer of that index and is still a
/// scaffold (its own issue), so this test writes the entry that create will.
const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// One endpoint type a client can discover its data plane with.
///
/// There is no per-type host form here: AWS names each type its own hostname, and
/// this emulator serves every one of those endpoints at the single endpoint it
/// answers on, so each type resolves to that one address — and a client built on
/// any of them reaches the plane its SigV4 scope names.
struct TypeCase {
    /// The `endpointType` the discovery call names.
    endpoint_type: &'static str,
    /// The thing this row drives the data plane with, so two rows cannot see
    /// each other's job executions.
    thing_name: &'static str,
}

/// `iot:Data-ATS` is the type AWS recommends (and the one the Jobs data plane is
/// reached through); `iot:Jobs` is the device-management Jobs API endpoint. Both
/// are in the catalog `DescribeEndpointRequest$endpointType` documents.
const TYPE_CASES: &[TypeCase] = &[
    TypeCase {
        endpoint_type: "iot:Data-ATS",
        thing_name: "discovery-probe-ats",
    },
    TypeCase {
        endpoint_type: "iot:Jobs",
        thing_name: "discovery-probe-jobs",
    },
];

/// A SigV4 credential scope naming `signing_name` in [`REGION`] — the signal the
/// router picks the owning service by.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

#[test]
fn a_discovered_iot_endpoint_address_serves_the_jobs_data_plane() {
    std::thread::Builder::new()
        .name("iot-endpoint-discovery".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            // The command the data-plane leg starts has to EXIST; see
            // [`COMMAND_ARN`].
            service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::command_registry(&*engine)
                .expect("the shared IoT command index")
                .lock()
                .register(COMMAND_ARN, service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::iot_commands::CommandFacts::default())
                .expect("register the command this test starts");
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            // Serve the emulator at an address a client can dial, as the core does
            // when its data port starts: that is the address discovery must report.
            router.set_url_base(
                EndpointUrl::parse(SERVED_AT).expect("the served base is a usable endpoint"),
            );

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let call = |method: &str, url: String, signing_name: &str, body: String| {
                let request = http::Request::builder()
                    .method(method)
                    .uri(url)
                    .header("authorization", authorization(signing_name))
                    .header("content-type", "application/json")
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let parsed: Value = serde_json::from_str(&rendered).unwrap_or(Value::Null);
                (status, rendered, parsed)
            };

            // `GET /endpoint?endpointType=…` on the control plane, returning the
            // address it reports.
            let discover = |endpoint_type: &str| -> String {
                let (status, rendered, parsed) = call(
                    "GET",
                    format!(
                        "https://{SERVED_AUTHORITY}/endpoint?endpointType={endpoint_type}"
                    ),
                    "iot",
                    String::new(),
                );
                assert_eq!(
                    status, 200,
                    "case {endpoint_type}: endpoint discovery must answer 200, \
                     got {status} with {rendered}"
                );
                parsed
                    .get("endpointAddress")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {endpoint_type}: no endpointAddress in {rendered}")
                    })
                    .to_string()
            };

            for case in TYPE_CASES {
                let endpoint_type = case.endpoint_type;
                let address = discover(endpoint_type);

                // The announced address is the one this emulator is served at: a
                // client that follows it reaches the emulator, not a host outside it.
                assert_eq!(
                    address, SERVED_AUTHORITY,
                    "case {endpoint_type}: discovery must announce the address this \
                     emulator is served at, expected {SERVED_AUTHORITY}, got {address}"
                );

                // "All subsequent calls to DescribeEndpoint return the same
                // endpoint": a client that discovers twice must be sent to one
                // host, or it would split its traffic.
                let again = discover(endpoint_type);
                assert_eq!(
                    again, address,
                    "case {endpoint_type}: a repeated discovery must report the same endpoint, \
                     expected {address}, got {again}"
                );

                // The second half of the bootstrap sequence: the Jobs data plane,
                // addressed at the host discovery just handed back. It must be
                // mounted there (not 404 UnknownService) and implemented (not 501
                // NotImplemented) — and it must be the SAME store across calls,
                // which is what the execution started here and read back proves.
                let (status, rendered, parsed) = call(
                    "POST",
                    format!("https://{address}/command-executions"),
                    "iot-jobs-data",
                    format!(
                        r#"{{"targetArn":"arn:aws:iot:{REGION}:111122223333:thing/{thing}","commandArn":"{COMMAND_ARN}"}}"#,
                        thing = case.thing_name
                    ),
                );
                assert_eq!(
                    status, 200,
                    "case {endpoint_type}: starting a command execution at the discovered host \
                     {address} must answer 200, got {status} with {rendered}"
                );
                let execution_id = parsed
                    .get("executionId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {endpoint_type}: no executionId in {rendered}")
                    })
                    .to_string();

                let (status, rendered, parsed) = call(
                    "GET",
                    format!("https://{address}/things/{}/jobs", case.thing_name),
                    "iot-jobs-data",
                    String::new(),
                );
                assert_eq!(
                    status, 200,
                    "case {endpoint_type}: the Jobs data plane at the discovered host {address} \
                     must answer 200, got {status} with {rendered}"
                );
                let queued: Vec<String> = parsed
                    .get("queuedJobs")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| {
                        panic!("case {endpoint_type}: no queuedJobs in {rendered}")
                    })
                    .iter()
                    .map(|summary| {
                        summary
                            .get("jobId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect();
                assert_eq!(
                    queued,
                    vec![execution_id.clone()],
                    "case {endpoint_type}: the execution started at {address} must be the one \
                     queued for {}, got {rendered}",
                    case.thing_name
                );
            }
        })
        .expect("spawn the endpoint-discovery thread")
        .join()
        .expect("a discovered IoT endpoint must serve the Jobs data plane");
}
