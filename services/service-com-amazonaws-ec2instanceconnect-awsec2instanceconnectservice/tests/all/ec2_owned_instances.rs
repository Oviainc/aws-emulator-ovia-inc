// SPDX-License-Identifier: CC0-1.0

//! Pushing a key at an instance AMAZON EC2 owns, driven end to end with both services on
//! one engine: EC2 over its `ec2Query` wire and EC2 Instance Connect over its own
//! `awsJson1_1` target.
//!
//! The rule under test: AN EC2 INSTANCE IS EC2'S RESOURCE, SO WHAT EC2 REPORTS ABOUT THE ID
//! A PUSH NAMES IS WHAT DECIDES THE PUSH — an instance EC2 holds is the instance the key is
//! pushed to, with the lifecycle state, the Availability Zone and the instance type EC2
//! reports for it, and an id EC2 does not hold is `EC2InstanceNotFoundException`. A registry
//! of this service's own cannot decide it: a key pushed at an instance that was launched
//! through `RunInstances` would be refused as unknown, and a key pushed at an id EC2 never
//! heard of would succeed.
//!
//! The rows span what that generalises over: every lifecycle state EC2 moves a launched
//! instance through (pending while it is launching, running, shutting-down after a
//! terminate), the zone EC2 placed it in beside one it did not, an instance type that runs
//! on the Nitro System beside one that does not, and the account's serial-console status as
//! EC2 reports it — none of them registered with this service.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_ec2instanceconnect_awsec2instanceconnectservice as ec2ic;

use super::instance_connect_key_push::{error_code, ssh_key, SEND_SERIAL, SEND_SSH};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The EC2 API version an `ec2Query` body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// The name pattern a client resolving an Amazon Linux 2 x86_64 gp2 AMI matches on — the
/// published name form of that variant — plus the `available` state, which is how every AWS
/// launch tutorial picks the AMI to launch.
const AL2_NAME_PATTERN: &str = "amzn2-ami-hvm-*-x86_64-gp2";

/// An instance type that runs on the AWS Nitro System, which is what the serial console
/// supports.
const NITRO_INSTANCE_TYPE: &str = "m5.large";

/// An instance type from before the Nitro System, which the serial console does not support.
const PRE_NITRO_INSTANCE_TYPE: &str = "m3.medium";

/// A well-formed instance id EC2 does not hold (`^i-[a-f0-9]+$`, 10..32 characters), so the
/// push is refused for the instance rather than for the shape of its id.
const UNKNOWN_INSTANCE_ID: &str = "i-0fedcba9876543210";

/// Long enough past the span EC2 owes a launched instance its next state in that the move is
/// certainly due.
fn past_the_transition() -> Duration {
    ec2::state::TRANSITION_DELAY + Duration::from_millis(400)
}

/// One emulator: a shared engine with BOTH services mounted on it, the way the router mounts
/// every service.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(ec2ic::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// One `ec2Query` call: an `application/x-www-form-urlencoded` POST whose body carries
    /// `Action=<Operation>`, answered with XML.
    async fn ec2(&self, case: &str, action: &str, params: &[(&str, &str)]) -> String {
        let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_value(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("ec2"))
            .body(Bytes::from(body))
            .expect("a well-formed ec2Query request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from EC2 {action}, got {status} with body \
             {answered}"
        );
        answered
    }

    /// One EC2 Instance Connect call: `POST /` with an `X-Amz-Target` and a JSON body.
    async fn connect(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("ec2-instance-connect.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header("authorization", authorization("ec2-instance-connect"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `SendSSHPublicKey` for one instance and OS user, with an Availability Zone only when
    /// the case names one.
    async fn push_ssh(
        &self,
        instance_id: &str,
        key: &str,
        availability_zone: Option<&str>,
    ) -> (u16, Value) {
        let mut body = json!({
            "InstanceId": instance_id,
            "InstanceOSUser": "ec2-user",
            "SSHPublicKey": key,
        });
        if let Some(zone) = availability_zone {
            body["AvailabilityZone"] = Value::String(zone.to_string());
        }
        self.connect(SEND_SSH, &body).await
    }

    /// This service's own store, for reading a stored key back the way a handler would.
    fn connect_state(
        &self,
    ) -> engine::ServiceState<
        ec2ic::state::ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState,
    > {
        ec2ic::state::service_state(&*self.engine).expect("the service state must be reachable")
    }

    /// Resolve the published Amazon Linux 2 AMI the documented way, launch one instance of
    /// `instance_type` from it, and answer the instance id and the Availability Zone EC2
    /// placed it in.
    async fn launch(&self, case: &str, instance_type: &str) -> (String, String) {
        let images = self
            .ec2(
                case,
                "DescribeImages",
                &[
                    ("Owner.1", "amazon"),
                    ("Filter.1.Name", "name"),
                    ("Filter.1.Value.1", AL2_NAME_PATTERN),
                    ("Filter.2.Name", "state"),
                    ("Filter.2.Value.1", "available"),
                ],
            )
            .await;
        let image_id = elements(&images, "imageId").into_iter().next().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the published catalog must report an available Amazon Linux 2 \
                 AMI for {AL2_NAME_PATTERN}, got {images}"
            )
        });

        let run = self
            .ec2(
                case,
                "RunInstances",
                &[
                    ("ImageId", image_id.as_str()),
                    ("InstanceType", instance_type),
                    ("MinCount", "1"),
                    ("MaxCount", "1"),
                ],
            )
            .await;
        let instance_id = elements(&run, "instanceId")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: RunInstances must mint an instance: {run}"));
        let zone = elements(&run, "availabilityZone")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the instance must report its zone: {run}"));
        (instance_id, zone)
    }
}

/// A SigV4 credential scope naming `signing_name`, which is how the router picks the service
/// out. The harness switches request authentication off, so the signature itself is never
/// verified and need not be real.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=00"
    )
}

/// Percent-encode the characters that are significant in an
/// `application/x-www-form-urlencoded` body.
fn form_value(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '+' => "%2B".to_string(),
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The text of every `<name>…</name>` element of an XML body, in document order.
fn elements(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        match rest.find(&close) {
            Some(end) => {
                found.push(rest[..end].to_string());
                rest = &rest[end + close.len()..];
            }
            None => break,
        }
    }
    found
}

/// The `Message` of an error body, or the empty string for a success.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A key pushed at an instance EC2 launched is served against EC2's own record of it: the
/// state EC2 reports decides the answer at every stage of the instance's life, and the zone
/// EC2 placed it in is the only zone the push accepts.
#[tokio::test]
async fn a_push_is_decided_by_what_ec2_reports_about_the_instance_it_launched() {
    let emulator = Emulator::new();
    let (instance_id, zone) = emulator.launch("a launched instance", NITRO_INSTANCE_TYPE).await;
    assert!(
        instance_id.starts_with("i-"),
        "the launch must answer an instance id, got {instance_id:?}"
    );

    // Launching: EC2 reports the instance `pending`, so the push is refused for the state it
    // is in — not as an unknown instance, and not with a success.
    let (status, body) = emulator
        .push_ssh(&instance_id, &ssh_key("while-pending"), None)
        .await;
    assert_eq!(
        error_code(&body),
        "EC2InstanceUnavailableException",
        "while pending: expected the refusal for an instance that is still launching, got \
         {status} {body}"
    );

    // One transition later EC2 reports it `running`, and the same push succeeds.
    tokio::time::sleep(past_the_transition()).await;
    let key = ssh_key("running");
    let (status, body) = emulator.push_ssh(&instance_id, &key, None).await;
    assert_eq!(
        status, 200,
        "once running: expected HTTP 200 from SendSSHPublicKey, got {status} {body}"
    );
    assert_eq!(
        body.get("Success").and_then(Value::as_bool),
        Some(true),
        "once running: expected Success true, got {body}"
    );

    // The key is really stored against that instance, so the success is not a canned reply.
    let stored = {
        let state = emulator.connect_state();
        let guard = state.lock();
        guard
            .active_ssh_key(&instance_id, "ec2-user", Instant::now())
            .map(|pushed| pushed.public_key.clone())
    };
    assert_eq!(
        stored.as_deref(),
        Some(key.as_str()),
        "once running: the pushed key must be readable from state, got {stored:?}"
    );

    // The zone EC2 placed it in is accepted; any other zone is a parameter that is not valid.
    let (status, body) = emulator
        .push_ssh(&instance_id, &ssh_key("right-zone"), Some(&zone))
        .await;
    assert_eq!(
        status, 200,
        "the zone EC2 reported: expected HTTP 200, got {status} {body}"
    );
    let other_zone = format!("{}z", zone.trim_end_matches(|c: char| c.is_ascii_alphabetic()));
    let (status, body) = emulator
        .push_ssh(&instance_id, &ssh_key("wrong-zone"), Some(&other_zone))
        .await;
    assert_eq!(
        error_code(&body),
        "InvalidArgsException",
        "zone {other_zone:?}: expected the refusal for a zone that is not the instance's, got \
         {status} {body}"
    );

    // Terminated through EC2: the push is refused for the state EC2 now reports, whichever
    // of the two shutting-down states the instance has reached.
    emulator
        .ec2(
            "a terminated instance",
            "TerminateInstances",
            &[("InstanceId.1", instance_id.as_str())],
        )
        .await;
    let (status, body) = emulator
        .push_ssh(&instance_id, &ssh_key("terminated"), None)
        .await;
    assert_eq!(
        error_code(&body),
        "EC2InstanceStateInvalidException",
        "after the terminate: expected the refusal for an instance that is going down, got \
         {status} {body}"
    );
}

/// An id EC2 does not hold is refused as not found, with the message AWS words that error
/// with — and no key is stored for it.
#[tokio::test]
async fn an_instance_ec2_does_not_hold_is_refused_as_not_found() {
    let emulator = Emulator::new();

    // Rows: an id nothing ever launched, and the id of an instance of this account's own
    // Region that EC2 was never asked to create (the seeded fixture ids of this service,
    // which EC2 does not hold either — with EC2 mounted, its answer is what decides).
    let seeded = ec2ic::state::seed_instances();
    let mut cases: Vec<String> = vec![UNKNOWN_INSTANCE_ID.to_string()];
    cases.extend(seeded.iter().map(|instance| instance.instance_id.clone()));

    for instance_id in cases {
        let (status, body) = emulator
            .push_ssh(&instance_id, &ssh_key("unknown"), None)
            .await;
        assert_eq!(
            error_code(&body),
            "EC2InstanceNotFoundException",
            "case {instance_id:?}: expected the refusal for an instance EC2 does not hold, \
             got {status} {body}"
        );
        assert_eq!(
            message(&body),
            "Instance not found.",
            "case {instance_id:?}: expected the message AWS words that error with, got {body}"
        );
        assert_eq!(
            emulator.connect_state().lock().pushed_keys.len(),
            0,
            "case {instance_id:?}: a refused push must store no key"
        );
    }
}

/// The serial console reads EC2 too: the account's status is the one EC2's own
/// `EnableSerialConsoleAccess` set, and whether the instance's type supports the console is
/// EC2's instance-type catalog's answer.
#[tokio::test]
async fn a_serial_console_push_reads_the_account_status_and_the_instance_type_from_ec2() {
    let emulator = Emulator::new();
    let (nitro_id, _) = emulator.launch("a Nitro instance", NITRO_INSTANCE_TYPE).await;
    let (pre_nitro_id, _) = emulator
        .launch("a pre-Nitro instance", PRE_NITRO_INSTANCE_TYPE)
        .await;
    tokio::time::sleep(past_the_transition()).await;

    let serial_body = |instance_id: &str, key: &str| {
        json!({
            "InstanceId": instance_id,
            "SerialPort": ec2ic::state::SERIAL_CONSOLE_PORT,
            "SSHPublicKey": key,
        })
    };

    // The account has not been authorized yet, so even the Nitro instance is refused.
    let (status, body) = emulator
        .connect(SEND_SERIAL, &serial_body(&nitro_id, &ssh_key("not-authorized")))
        .await;
    assert_eq!(
        error_code(&body),
        "SerialConsoleAccessDisabledException",
        "before EnableSerialConsoleAccess: expected the refusal for an account that is not \
         authorized, got {status} {body}"
    );

    // Authorizing it is EC2's own operation, and EC2's status is what the push reads.
    emulator
        .ec2("authorizing the account", "EnableSerialConsoleAccess", &[])
        .await;
    let key = ssh_key("serial");
    let (status, body) = emulator
        .connect(SEND_SERIAL, &serial_body(&nitro_id, &key))
        .await;
    assert_eq!(
        status, 200,
        "after EnableSerialConsoleAccess: expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        body.get("Success").and_then(Value::as_bool),
        Some(true),
        "after EnableSerialConsoleAccess: expected Success true, got {body}"
    );
    let stored = {
        let state = emulator.connect_state();
        let guard = state.lock();
        guard
            .active_serial_console_key(&nitro_id, ec2ic::state::SERIAL_CONSOLE_PORT, Instant::now())
            .map(|pushed| pushed.public_key.clone())
    };
    assert_eq!(
        stored.as_deref(),
        Some(key.as_str()),
        "after EnableSerialConsoleAccess: the pushed key must be readable from state, got \
         {stored:?}"
    );

    // The instance type decides the rest: a type that does not run on the Nitro System is
    // refused even though the account is authorized and the instance is running.
    let (status, body) = emulator
        .connect(
            SEND_SERIAL,
            &serial_body(&pre_nitro_id, &ssh_key("pre-nitro")),
        )
        .await;
    assert_eq!(
        error_code(&body),
        "EC2InstanceTypeInvalidException",
        "case {PRE_NITRO_INSTANCE_TYPE:?}: expected the refusal for an instance type the \
         serial console does not support, got {status} {body}"
    );
}
