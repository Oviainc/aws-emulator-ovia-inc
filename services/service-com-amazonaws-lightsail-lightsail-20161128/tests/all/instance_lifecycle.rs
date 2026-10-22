// SPDX-License-Identifier: CC0-1.0

//! The Lightsail instance lifecycle — create, read, list, stop, start, delete —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: each of these operations answers from the ONE persisted
//! instance record, so anything `CreateInstances` stored is what a later
//! `GetInstance`, `GetInstances` or `GetInstanceState` reports, every
//! server-assigned member is computed from the request plus the store's own
//! allocators, a `StopInstance`/`StartInstance` is observable through the next
//! read, and a name that was never created — or has since been deleted — is
//! refused with the service's `DoesNotExist` envelope (pinned in
//! `instance_error_envelope.rs`).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// One emulator with one Lightsail service mounted on one shared engine.
///
/// `pub(crate)` so a sibling suite in this test target drives the service through the
/// same front door rather than standing up a second harness.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` and require success, answering the response body.
    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {parsed:?}");
        parsed
    }

    /// The stored instance `name`, as `GetInstance` reports it.
    async fn instance(&self, name: &str) -> Value {
        self.ok("GetInstance", json!({"instanceName": name})).await["instance"].clone()
    }

    /// Every instance name `GetInstances` reports, walking every page.
    ///
    /// Answers the names in page order together with the size of each page, so a
    /// caller can assert both the contents and the paging.
    async fn all_instance_names(&self) -> (Vec<String>, Vec<usize>) {
        let mut names = Vec::new();
        let mut page_sizes = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let body = match &token {
                Some(token) => self.ok("GetInstances", json!({"pageToken": token})).await,
                None => self.ok("GetInstances", json!({})).await,
            };
            let page = body["instances"]
                .as_array()
                .unwrap_or_else(|| panic!("case listing instances: expected an instances array, got {body:?}"))
                .clone();
            page_sizes.push(page.len());
            for instance in page {
                names.push(
                    instance["name"]
                        .as_str()
                        .expect("every listed instance carries a name")
                        .to_string(),
                );
            }
            match body["nextPageToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(
                page_sizes.len() < 100,
                "case listing instances: the walk did not terminate after {} pages",
                page_sizes.len()
            );
        }
        (names, page_sizes)
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
///
/// `pub(crate)` so a sibling suite in this test target reads the code the same way.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Whether `value` has the shape of the UUID Lightsail's instance and operation
/// ids take: five hex groups of 8-4-4-4-12.
fn is_uuid_shaped(value: &str) -> bool {
    let lengths: Vec<usize> = value.split('-').map(str::len).collect();
    lengths == vec![8, 4, 4, 4, 12]
        && value.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

/// Whether `value` satisfies the modelled `Ipv6Address` pattern
/// `^([A-F0-9]{1,4}:){7}[A-F0-9]{1,4}$`.
fn is_modelled_ipv6(value: &str) -> bool {
    let groups: Vec<&str> = value.split(':').collect();
    groups.len() == 8
        && groups.iter().all(|group| {
            (1..=4).contains(&group.len())
                && group
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
        })
}

/// Whether `value` satisfies the modelled `IpAddress` pattern
/// `^([0-9]{1,3}\.){3}[0-9]{1,3}$`.
fn is_modelled_ipv4(value: &str) -> bool {
    let groups: Vec<&str> = value.split('.').collect();
    groups.len() == 4
        && groups
            .iter()
            .all(|group| (1..=3).contains(&group.len()) && group.chars().all(char::is_numeric))
}

/// Poll `GetInstanceState` until it reports `want`, answering the state.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let body = emulator
            .ok("GetInstanceState", json!({"instanceName": name}))
            .await;
        if body["state"]["name"].as_str() == Some(want) {
            return body["state"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            body["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The create/read table
// ---------------------------------------------------------------------------

/// One `CreateInstances` request, and what the rule says the stored instance then
/// reports. The rows differ along everything the rule generalises over: the
/// availability zone (and so the region in the ARN and the location), the
/// blueprint (and so the friendly name, the login user and the firewall's
/// management port), the bundle (and so the hardware and the monthly transfer),
/// the IP address type (and so which addresses exist), the key pair, and the
/// number of tags.
struct Row {
    case: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    blueprint_id: &'static str,
    blueprint_name: &'static str,
    /// `None` where this service publishes no default user name for the
    /// blueprint family, in which case the member must be absent rather than
    /// guessed.
    username: Option<&'static str>,
    bundle_id: &'static str,
    cpu_count: i64,
    ram_size_in_gb: f64,
    disk_size_in_gb: i64,
    transfer_gb_per_month: i64,
    /// What the request sends, or `None` to send no `ipAddressType` at all.
    ip_address_type: Option<&'static str>,
    /// What the instance must then report (`dualstack` is the modelled default).
    effective_ip_address_type: &'static str,
    key_pair_name: Option<&'static str>,
    ssh_key_name: &'static str,
    tags: &'static [(&'static str, &'static str)],
    /// The management port the default firewall opens: SSH for Linux/Unix, RDP
    /// for Windows.
    management_port: i64,
}

/// The table. Bundle figures are `crate::state::BUNDLE_SPECS`' published plans
/// and user names are the Lightsail "Connecting to your instance" defaults, both
/// recorded in `src/state.rs`; the issue's own case (`amazon_linux_2023` on
/// `micro_3_0` in `us-east-1a`) is only the first row.
const ROWS: &[Row] = &[
    Row {
        case: "amazon linux, no ipAddressType and no key pair",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        blueprint_name: "Amazon Linux 2023",
        username: Some("ec2-user"),
        bundle_id: "micro_3_0",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_gb_per_month: 2_048,
        ip_address_type: None,
        effective_ip_address_type: "dualstack",
        key_pair_name: None,
        ssh_key_name: "LightsailDefaultKeyPair",
        tags: &[],
        management_port: 22,
    },
    Row {
        case: "ubuntu, ipv4 only, one tag, own key pair, smallest bundle",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        blueprint_id: "ubuntu_22_04",
        blueprint_name: "Ubuntu 22 04",
        username: Some("ubuntu"),
        bundle_id: "nano_3_0",
        cpu_count: 1,
        ram_size_in_gb: 0.5,
        disk_size_in_gb: 20,
        transfer_gb_per_month: 1_024,
        ip_address_type: Some("ipv4"),
        effective_ip_address_type: "ipv4",
        key_pair_name: Some("team-key"),
        ssh_key_name: "team-key",
        tags: &[("env", "test")],
        management_port: 22,
    },
    Row {
        case: "windows, ipv6 only, two tags",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        blueprint_id: "windows_server_2022",
        blueprint_name: "Windows Server 2022",
        username: Some("Administrator"),
        bundle_id: "large_3_0",
        cpu_count: 2,
        ram_size_in_gb: 8.0,
        disk_size_in_gb: 160,
        transfer_gb_per_month: 5_120,
        ip_address_type: Some("ipv6"),
        effective_ip_address_type: "ipv6",
        key_pair_name: None,
        ssh_key_name: "LightsailDefaultKeyPair",
        tags: &[("owner", "platform"), ("tier", "gold")],
        management_port: 3389,
    },
    Row {
        case: "bitnami application blueprint, largest bundle in the table",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        blueprint_id: "wordpress",
        blueprint_name: "Wordpress",
        username: Some("bitnami"),
        bundle_id: "4xlarge_3_0",
        cpu_count: 16,
        ram_size_in_gb: 64.0,
        disk_size_in_gb: 1_280,
        transfer_gb_per_month: 8_192,
        ip_address_type: Some("dualstack"),
        effective_ip_address_type: "dualstack",
        key_pair_name: None,
        ssh_key_name: "LightsailDefaultKeyPair",
        tags: &[],
        management_port: 22,
    },
    Row {
        case: "bundle id carrying a platform token, so the size is not the whole id",
        availability_zone: "sa-east-1a",
        region: "sa-east-1",
        blueprint_id: "windows_server_2019",
        blueprint_name: "Windows Server 2019",
        username: Some("Administrator"),
        bundle_id: "micro_win_2_0",
        cpu_count: 1,
        ram_size_in_gb: 1.0,
        disk_size_in_gb: 40,
        transfer_gb_per_month: 2_048,
        ip_address_type: Some("ipv4"),
        effective_ip_address_type: "ipv4",
        key_pair_name: None,
        ssh_key_name: "LightsailDefaultKeyPair",
        tags: &[],
        management_port: 3389,
    },
    Row {
        case: "blueprint family with no published default user name",
        availability_zone: "ca-central-1a",
        region: "ca-central-1",
        blueprint_id: "plesk_ubuntu_18_0_35",
        blueprint_name: "Plesk Ubuntu 18 0 35",
        username: None,
        bundle_id: "medium_3_0",
        cpu_count: 2,
        ram_size_in_gb: 4.0,
        disk_size_in_gb: 80,
        transfer_gb_per_month: 4_096,
        ip_address_type: Some("dualstack"),
        effective_ip_address_type: "dualstack",
        key_pair_name: None,
        ssh_key_name: "LightsailDefaultKeyPair",
        tags: &[],
        management_port: 22,
    },
];

/// The `CreateInstances` body for `row`, naming one instance.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = json!({
        "instanceNames": [name],
        "availabilityZone": row.availability_zone,
        "blueprintId": row.blueprint_id,
        "bundleId": row.bundle_id,
    });
    if let Some(ip_address_type) = row.ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    if let Some(key_pair_name) = row.key_pair_name {
        body["keyPairName"] = json!(key_pair_name);
    }
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    body
}

// ---------------------------------------------------------------------------
// Create and read back
// ---------------------------------------------------------------------------

/// RULE: every member `GetInstance` reports comes from the one stored record —
/// the caller's own name, zone, blueprint, bundle, key pair and tags verbatim,
/// and every server-assigned member computed from them plus the store's
/// allocators.
#[tokio::test]
async fn an_instance_reads_back_as_it_was_created() {
    let emulator = Emulator::new();
    let mut addresses: BTreeSet<String> = BTreeSet::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("row-{index}");

        let created = emulator
            .ok("CreateInstances", create_body(row, &name))
            .await;
        let operations = created["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {created:?}"));
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected one operation per created instance, got {operations:?}"
        );
        let operation = &operations[0];
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the operation to name the instance, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("Instance"),
            "case {case}: expected resourceType Instance, got {operation:?}"
        );
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateInstance"),
            "case {case}: expected operationType CreateInstance, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Started"),
            "case {case}: a create that is not finished yet reports Started, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: a create that is not finished yet is not terminal, got {operation:?}"
        );
        let operation_id = operation["id"].as_str().unwrap_or_default();
        assert!(
            is_uuid_shaped(operation_id),
            "case {case}: expected a uuid-shaped operation id, got {operation_id:?}"
        );
        assert_eq!(
            operation["location"]["availabilityZone"].as_str(),
            Some(row.availability_zone),
            "case {case}: expected the operation's zone to be the requested one, got {operation:?}"
        );

        let instance = emulator.instance(&name).await;

        assert_eq!(
            instance["name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the requested name, got {instance:?}"
        );
        assert_eq!(
            instance["resourceType"].as_str(),
            Some("Instance"),
            "case {case}: expected resourceType Instance, got {instance:?}"
        );
        assert_eq!(
            instance["blueprintId"].as_str(),
            Some(row.blueprint_id),
            "case {case}: expected the requested blueprintId, got {instance:?}"
        );
        assert_eq!(
            instance["blueprintName"].as_str(),
            Some(row.blueprint_name),
            "case {case}: expected the blueprint's friendly name, got {:?}",
            instance["blueprintName"]
        );
        assert_eq!(
            instance["bundleId"].as_str(),
            Some(row.bundle_id),
            "case {case}: expected the requested bundleId, got {instance:?}"
        );
        assert_eq!(
            instance["location"]["availabilityZone"].as_str(),
            Some(row.availability_zone),
            "case {case}: expected the requested availabilityZone, got {:?}",
            instance["location"]
        );
        assert_eq!(
            instance["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the zone's own region, got {:?}",
            instance["location"]
        );

        // The ARN carries the zone's region and the canonical emulator account,
        // and ends in a uuid no other instance is using.
        let arn = instance["arn"].as_str().unwrap_or_default().to_string();
        let prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:Instance/", row.region);
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an ARN starting '{prefix}', got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped ARN id, got {id:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case}: expected a fresh ARN, got the already-used {arn:?}"
        );
        assert_eq!(
            instance["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: expected the account/resource-id support code, got {:?}",
            instance["supportCode"]
        );
        assert!(
            instance["createdAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a creation timestamp, got {:?}",
            instance["createdAt"]
        );

        assert_eq!(
            instance["sshKeyName"].as_str(),
            Some(row.ssh_key_name),
            "case {case}: expected the key pair the request named, got {instance:?}"
        );
        assert_eq!(
            instance["username"].as_str(),
            row.username,
            "case {case}: expected the blueprint's login user, got {:?}",
            instance["username"]
        );
        assert_eq!(
            instance["isStaticIp"].as_bool(),
            Some(false),
            "case {case}: nothing attached a static IP, got {instance:?}"
        );

        // Tags: exactly the pairs the request carried, in no particular order.
        let reported_tags: BTreeSet<(String, String)> = instance["tags"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a tags array, got {instance:?}"))
            .iter()
            .map(|tag| {
                (
                    tag["key"].as_str().unwrap_or_default().to_string(),
                    tag["value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let requested_tags: BTreeSet<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags, requested_tags,
            "case {case}: expected the requested tags, got {:?}",
            instance["tags"]
        );

        // Addresses: present exactly for the address families the IP address type
        // gives the instance, and never shared with another instance.
        assert_eq!(
            instance["ipAddressType"].as_str(),
            Some(row.effective_ip_address_type),
            "case {case}: expected the effective ipAddressType, got {instance:?}"
        );
        let wants_ipv4 = matches!(row.effective_ip_address_type, "ipv4" | "dualstack");
        let wants_ipv6 = matches!(row.effective_ip_address_type, "ipv6" | "dualstack");
        for member in ["publicIpAddress", "privateIpAddress"] {
            match instance[member].as_str() {
                Some(address) => {
                    assert!(
                        wants_ipv4,
                        "case {case}: expected no {member} for {}, got {address:?}",
                        row.effective_ip_address_type
                    );
                    assert!(
                        is_modelled_ipv4(address),
                        "case {case}: expected a dotted-quad {member}, got {address:?}"
                    );
                    assert!(
                        addresses.insert(address.to_string()),
                        "case {case}: expected a fresh {member}, got the already-used {address:?}"
                    );
                }
                None => assert!(
                    !wants_ipv4,
                    "case {case}: expected a {member} for {}, got {:?}",
                    row.effective_ip_address_type, instance[member]
                ),
            }
        }
        let ipv6 = instance["ipv6Addresses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an ipv6Addresses array, got {instance:?}"));
        assert_eq!(
            !ipv6.is_empty(),
            wants_ipv6,
            "case {case}: expected ipv6Addresses to be populated only for {}, got {ipv6:?}",
            row.effective_ip_address_type
        );
        for address in ipv6 {
            let address = address.as_str().unwrap_or_default();
            assert!(
                is_modelled_ipv6(address),
                "case {case}: expected a modelled IPv6 address, got {address:?}"
            );
            assert!(
                addresses.insert(address.to_string()),
                "case {case}: expected a fresh IPv6 address, got the already-used {address:?}"
            );
        }
        assert_eq!(
            instance["metadataOptions"]["httpProtocolIpv6"].as_str(),
            Some(if wants_ipv6 { "enabled" } else { "disabled" }),
            "case {case}: expected the metadata IPv6 endpoint to follow the address type, got {:?}",
            instance["metadataOptions"]
        );

        // Hardware and networking come from the bundle the request named.
        let hardware = &instance["hardware"];
        assert_eq!(
            hardware["cpuCount"].as_i64(),
            Some(row.cpu_count),
            "case {case}: expected the bundle's vCPU count, got {hardware:?}"
        );
        assert_eq!(
            hardware["ramSizeInGb"].as_f64(),
            Some(row.ram_size_in_gb),
            "case {case}: expected the bundle's RAM, got {hardware:?}"
        );
        let disks = hardware["disks"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a disks array, got {hardware:?}"));
        assert_eq!(
            disks.len(),
            1,
            "case {case}: expected only the bundle's system disk, got {disks:?}"
        );
        assert_eq!(
            disks[0]["sizeInGb"].as_i64(),
            Some(row.disk_size_in_gb),
            "case {case}: expected the bundle's SSD size, got {:?}",
            disks[0]
        );
        assert_eq!(
            disks[0]["isSystemDisk"].as_bool(),
            Some(true),
            "case {case}: expected the bundle's disk to be the system disk, got {:?}",
            disks[0]
        );
        assert_eq!(
            instance["networking"]["monthlyTransfer"]["gbPerMonthAllocated"].as_i64(),
            Some(row.transfer_gb_per_month),
            "case {case}: expected the bundle's monthly transfer, got {:?}",
            instance["networking"]
        );

        // The default firewall opens the platform's management port and HTTP.
        let ports: BTreeSet<i64> = instance["networking"]["ports"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a ports array, got {instance:?}"))
            .iter()
            .map(|port| port["fromPort"].as_i64().unwrap_or_default())
            .collect();
        assert_eq!(
            ports,
            BTreeSet::from([row.management_port, 80]),
            "case {case}: expected the platform's management port and HTTP, got {:?}",
            instance["networking"]["ports"]
        );

        // A brand-new instance is not serving yet.
        assert_eq!(
            instance["state"]["name"].as_str(),
            Some("pending"),
            "case {case}: expected a new instance to be pending, got {:?}",
            instance["state"]
        );
        assert_eq!(
            instance["state"]["code"].as_i64(),
            Some(0),
            "case {case}: expected the pending state code, got {:?}",
            instance["state"]
        );
    }

    // Every row's instance is listed, and nothing else is.
    let (listed, _) = emulator.all_instance_names().await;
    let expected: Vec<String> = (0..ROWS.len()).map(|index| format!("row-{index}")).collect();
    assert_eq!(
        listed, expected,
        "case listing every created instance: expected {expected:?}, got {listed:?}"
    );
}

/// RULE: a created instance is answered in a non-terminal state and the clock
/// owes it `running`; the span is never zero, so a caller reading straight after
/// the create still sees the transient state.
#[tokio::test]
async fn the_clock_owes_a_new_instance_its_running_state() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateInstances", create_body(&ROWS[0], "awaited"))
        .await;

    let first = emulator
        .ok("GetInstanceState", json!({"instanceName": "awaited"}))
        .await;
    assert_eq!(
        first["state"]["name"].as_str(),
        Some("pending"),
        "case straight after create: expected pending, got {:?}",
        first["state"]
    );

    let settled = wait_for_state(&emulator, "awaited", "running").await;
    assert_eq!(
        settled["code"].as_i64(),
        Some(16),
        "case after the clock settled it: expected the running state code, got {settled:?}"
    );
    // The state the clock settled is the SAME state `GetInstance` reports: both
    // read the one record.
    let instance = emulator.instance("awaited").await;
    assert_eq!(
        instance["state"], settled,
        "case reading the whole instance: expected the same state GetInstanceState reports, got {:?}",
        instance["state"]
    );
}

// ---------------------------------------------------------------------------
// Stop and start
// ---------------------------------------------------------------------------

/// RULE: `StopInstance` and `StartInstance` move the stored state, so the next
/// read observes the change — and `force` is what makes a stop complete as the
/// call returns instead of passing through `stopping`.
#[tokio::test]
async fn stopping_and_starting_move_the_stored_state() {
    for (case, force) in [("an ordinary stop", false), ("a forced stop", true)] {
        let emulator = Emulator::new();
        let name = "cycled";
        emulator
            .ok("CreateInstances", create_body(&ROWS[0], name))
            .await;
        wait_for_state(&emulator, name, "running").await;

        let mut stop = json!({"instanceName": name});
        if force {
            stop["force"] = json!(true);
        }
        let stopped = emulator.ok("StopInstance", stop).await;
        let operation = &stopped["operations"][0];
        assert_eq!(
            operation["operationType"].as_str(),
            Some("StopInstance"),
            "case {case}: expected operationType StopInstance, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(name),
            "case {case}: expected the operation to name the instance, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(force),
            "case {case}: a forced stop is already done and an ordinary one is not, got {operation:?}"
        );

        let straight_after = emulator
            .ok("GetInstanceState", json!({"instanceName": name}))
            .await;
        let expected_straight_after = if force { "stopped" } else { "stopping" };
        assert_eq!(
            straight_after["state"]["name"].as_str(),
            Some(expected_straight_after),
            "case {case}, read straight after: expected {expected_straight_after}, got {:?}",
            straight_after["state"]
        );

        let settled = wait_for_state(&emulator, name, "stopped").await;
        assert_eq!(
            settled["code"].as_i64(),
            Some(80),
            "case {case}: expected the stopped state code, got {settled:?}"
        );
        // The whole instance reports the same state: `GetInstance` and
        // `GetInstanceState` read one record, not two.
        assert_eq!(
            emulator.instance(name).await["state"],
            settled,
            "case {case}: expected GetInstance to report the stopped state too"
        );

        // Starting it again is owed `running` the same way a create is.
        let started = emulator
            .ok("StartInstance", json!({"instanceName": name}))
            .await;
        assert_eq!(
            started["operations"][0]["operationType"].as_str(),
            Some("StartInstance"),
            "case {case}: expected operationType StartInstance, got {:?}",
            started["operations"]
        );
        let straight_after = emulator
            .ok("GetInstanceState", json!({"instanceName": name}))
            .await;
        assert_eq!(
            straight_after["state"]["name"].as_str(),
            Some("pending"),
            "case {case}, read straight after the start: expected pending, got {:?}",
            straight_after["state"]
        );
        let running = wait_for_state(&emulator, name, "running").await;
        assert_eq!(
            running["code"].as_i64(),
            Some(16),
            "case {case}: expected the running state code, got {running:?}"
        );
    }
}

/// RULE: starting an instance that is already running leaves the stored state
/// where it is — a mutation only moves what it has somewhere to move.
#[tokio::test]
async fn starting_a_running_instance_leaves_its_state_alone() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateInstances", create_body(&ROWS[0], "already-up"))
        .await;
    wait_for_state(&emulator, "already-up", "running").await;

    emulator
        .ok("StartInstance", json!({"instanceName": "already-up"}))
        .await;
    let state = emulator
        .ok("GetInstanceState", json!({"instanceName": "already-up"}))
        .await;
    assert_eq!(
        state["state"]["name"].as_str(),
        Some("running"),
        "case starting an already-running instance: expected it to stay running, got {:?}",
        state["state"]
    );
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// RULE: a delete removes the one record, so every later read of that name is
/// refused as `DoesNotExist` and the listing no longer carries it — while every
/// other instance is untouched.
#[tokio::test]
async fn deleting_an_instance_removes_it_from_every_read() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["keep-a", "doomed", "keep-b"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
        )
        .await;

    let deleted = emulator
        .ok("DeleteInstance", json!({"instanceName": "doomed"}))
        .await;
    let operation = &deleted["operations"][0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some("DeleteInstance"),
        "case deleting an instance: expected operationType DeleteInstance, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case deleting an instance: the record is gone as the call returns, got {operation:?}"
    );

    let (status, body) = emulator
        .call("GetInstance", json!({"instanceName": "doomed"}))
        .await;
    assert_eq!(
        status, 400,
        "case reading a deleted instance: expected 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "case reading a deleted instance: expected NotFoundException, got {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some("DoesNotExist"),
        "case reading a deleted instance: expected code DoesNotExist, got {body:?}"
    );

    let (status, body) = emulator
        .call("DeleteInstance", json!({"instanceName": "doomed"}))
        .await;
    assert_eq!(
        status, 400,
        "case deleting the same instance twice: expected 400, got {status} {body:?}"
    );

    let (listed, _) = emulator.all_instance_names().await;
    assert_eq!(
        listed,
        vec!["keep-a".to_string(), "keep-b".to_string()],
        "case listing after a delete: expected only the instances that were kept, got {listed:?}"
    );
}

/// RULE: a well-formed name that names no stored instance is refused with
/// Lightsail's `DoesNotExist` envelope on every operation that takes one — never a
/// 501 and never a hollow 200. The envelope itself (status, code and message) is
/// pinned over a table of names in `instance_error_envelope.rs`.
#[tokio::test]
async fn an_unknown_instance_does_not_exist_everywhere() {
    let emulator = Emulator::new();
    for operation in [
        "GetInstance",
        "GetInstanceState",
        "StartInstance",
        "StopInstance",
        "DeleteInstance",
    ] {
        let (status, body) = emulator
            .call(operation, json!({"instanceName": "never-created"}))
            .await;
        assert_eq!(
            status, 400,
            "case {operation} on an unknown name: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {operation} on an unknown name: expected NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {operation} on an unknown name: expected code DoesNotExist, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a create that cannot be served stores nothing and is refused with the
/// modelled `InvalidInputException` (400), naming the member that is wrong.
#[tokio::test]
async fn an_unservable_create_is_refused_and_stores_nothing() {
    // Each row is one way the request is unacceptable: the bundle size, the
    // availability zone, the IP address type, the count of names, a name
    // repeated within one request, and more add-ons than the store accepts.
    // A member outside a MODELLED constraint is refused by the centralized walk
    // before the handler runs, which is why the add-on-type row expects that
    // refusal's own code rather than this service's.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a bundle size this service does not serve",
            json!({
                "instanceNames": ["refused"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "titanic_9_0",
            }),
            "InvalidInputException",
        ),
        (
            "an availability zone outside every Lightsail region",
            json!({
                "instanceNames": ["refused"],
                "availabilityZone": "mars-west-9z",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
            "InvalidInputException",
        ),
        (
            "an availability zone with no zone letter",
            json!({
                "instanceNames": ["refused"],
                "availabilityZone": "us-east-1",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
            "InvalidInputException",
        ),
        (
            "no instance names at all",
            json!({
                "instanceNames": [],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
            "InvalidInputException",
        ),
        (
            "the same name twice in one request",
            json!({
                "instanceNames": ["twice", "twice"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
            "InvalidInputException",
        ),
        (
            "more add-ons than the store accepts",
            json!({
                "instanceNames": ["refused"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
                "addOns": (0..9)
                    .map(|_| json!({"addOnType": "AutoSnapshot"}))
                    .collect::<Vec<Value>>(),
            }),
            "InvalidInputException",
        ),
        (
            "an add-on type outside the modelled enum",
            json!({
                "instanceNames": ["refused"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
                "addOns": [{"addOnType": "TeaMaker"}],
            }),
            "ValidationException",
        ),
        (
            "an ipAddressType outside the modelled enum",
            json!({
                "instanceNames": ["refused"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
                "ipAddressType": "ipv5",
            }),
            "ValidationException",
        ),
    ];

    for (case, body, expected_code) in rows {
        let emulator = Emulator::new();
        let (status, answered) = emulator.call("CreateInstances", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case}: expected {expected_code}, got {answered:?}"
        );
        let (listed, _) = emulator.all_instance_names().await;
        assert!(
            listed.is_empty(),
            "case {case}: a refused create must store nothing, got {listed:?}"
        );
    }
}

/// RULE: a name already in use is refused, the existing record is untouched, and
/// no other name in the same request is created either.
#[tokio::test]
async fn an_existing_name_cannot_be_taken_again() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateInstances", create_body(&ROWS[1], "taken"))
        .await;
    let before = emulator.instance("taken").await;

    let (status, body) = emulator
        .call(
            "CreateInstances",
            json!({
                "instanceNames": ["fresh", "taken"],
                "availabilityZone": "eu-central-1a",
                "blueprintId": "debian_12",
                "bundleId": "small_3_0",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case re-creating an existing name: expected 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case re-creating an existing name: expected InvalidInputException, got {body:?}"
    );
    assert_eq!(
        emulator.instance("taken").await,
        before,
        "case re-creating an existing name: the stored record must be untouched"
    );
    let (listed, _) = emulator.all_instance_names().await;
    assert_eq!(
        listed,
        vec!["taken".to_string()],
        "case re-creating an existing name: the other name in the request must not be created, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Add-ons, user data and listing
// ---------------------------------------------------------------------------

/// RULE: an add-on the request enables is stored on the instance and reported by
/// every later read, carrying the parameters its own request structure supplied.
#[tokio::test]
async fn add_ons_are_stored_on_the_instance() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["with-add-ons"],
                "availabilityZone": "eu-west-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "small_3_0",
                "userData": "#!/bin/sh\nyum -y update\n",
                "addOns": [
                    {
                        "addOnType": "AutoSnapshot",
                        "autoSnapshotAddOnRequest": {"snapshotTimeOfDay": "06:00"},
                    },
                    {
                        "addOnType": "StopInstanceOnIdle",
                        "stopInstanceOnIdleRequest": {"threshold": "5", "duration": "20"},
                    },
                ],
            }),
        )
        .await;

    let add_ons = emulator.instance("with-add-ons").await["addOns"].clone();
    let add_ons = add_ons
        .as_array()
        .unwrap_or_else(|| panic!("case reading add-ons: expected an array, got {add_ons:?}"));
    assert_eq!(
        add_ons.len(),
        2,
        "case reading add-ons: expected both requested add-ons, got {add_ons:?}"
    );
    let auto = add_ons
        .iter()
        .find(|add_on| add_on["name"].as_str() == Some("AutoSnapshot"))
        .unwrap_or_else(|| panic!("case reading add-ons: expected an AutoSnapshot, got {add_ons:?}"));
    assert_eq!(
        auto["snapshotTimeOfDay"].as_str(),
        Some("06:00"),
        "case reading add-ons: expected the requested snapshot hour, got {auto:?}"
    );
    assert_eq!(
        auto["status"].as_str(),
        Some("Enabled"),
        "case reading add-ons: an add-on the request enabled is enabled, got {auto:?}"
    );
    let idle = add_ons
        .iter()
        .find(|add_on| add_on["name"].as_str() == Some("StopInstanceOnIdle"))
        .unwrap_or_else(|| {
            panic!("case reading add-ons: expected a StopInstanceOnIdle, got {add_ons:?}")
        });
    assert_eq!(
        (idle["threshold"].as_str(), idle["duration"].as_str()),
        (Some("5"), Some("20")),
        "case reading add-ons: expected the requested idle parameters, got {idle:?}"
    );

    // An `AutoSnapshot` with no hour named still reports one, and reports the
    // SAME one on every read.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["default-hour"],
                "availabilityZone": "eu-west-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "small_3_0",
                "addOns": [{"addOnType": "AutoSnapshot"}],
            }),
        )
        .await;
    let first = emulator.instance("default-hour").await["addOns"][0]["snapshotTimeOfDay"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        first.len() == 5 && first.ends_with(":00") && first[..2].parse::<u8>().is_ok_and(|hour| hour < 24),
        "case an AutoSnapshot with no hour: expected an HH:00 hour, got {first:?}"
    );
    let second = emulator.instance("default-hour").await["addOns"][0]["snapshotTimeOfDay"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        first, second,
        "case an AutoSnapshot with no hour: the hour must be stable across reads"
    );
}

/// RULE: `GetInstances` enumerates what is actually stored — nothing on an empty
/// store, and every created instance exactly once across the pages, in the
/// store's own order, however many there are.
#[tokio::test]
async fn listing_instances_enumerates_the_store_without_repeating() {
    let emulator = Emulator::new();

    let empty = emulator.ok("GetInstances", json!({})).await;
    assert_eq!(
        empty["instances"].as_array().map(Vec::len),
        Some(0),
        "case an empty store: expected no instances, got {empty:?}"
    );
    assert!(
        empty["nextPageToken"].is_null(),
        "case an empty store: expected no next page, got {empty:?}"
    );

    // Enough instances that the walk spans several pages.
    let names: Vec<String> = (0..25).map(|index| format!("paged-{index:02}")).collect();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": names,
                "availabilityZone": "us-west-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;

    let (listed, page_sizes) = emulator.all_instance_names().await;
    let mut expected = names.clone();
    expected.sort();
    assert_eq!(
        listed, expected,
        "case 25 instances: expected every name exactly once in store order, got {listed:?}"
    );
    assert!(
        page_sizes.len() > 1,
        "case 25 instances: expected the walk to span several pages, got pages {page_sizes:?}"
    );
    assert!(
        page_sizes.iter().all(|size| *size <= 10),
        "case 25 instances: expected no page past the page size, got pages {page_sizes:?}"
    );
}
