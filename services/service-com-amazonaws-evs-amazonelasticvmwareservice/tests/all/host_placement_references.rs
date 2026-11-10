// SPDX-License-Identifier: CC0-1.0

//! What a host request may NAME: the EC2 resources it is placed on, and the ESX version
//! it runs.
//!
//! An Amazon EVS host IS an EC2 bare metal instance ("An ESX host that runs on an Amazon
//! EC2 bare metal instance", `Host` doc), and the resources it is PLACED on belong to the
//! caller's own account — `placementGroupId` is "The unique ID of the placement group
//! where the host is placed" and `dedicatedHostId` "The unique ID of the Amazon EC2
//! Dedicated Host" (`HostInfoForCreate` member docs). Amazon EVS creates neither, so the
//! rules under test are:
//!
//! * a reference to an EC2 resource the account's own Amazon EC2 store holds is accepted
//!   and reported back on every later read of the host, and
//! * a reference the account does not hold is refused with the operation's
//!   `ValidationException`, whose `fieldList` names the member that named it, and nothing
//!   is stored and no instance launched; and
//! * an `esxVersion` the request names is one of the versions Amazon EVS offers for the
//!   environment's VCF version — "To find the available ESX versions for a particular VCF
//!   version, use the GetVersions action" (`CreateEnvironmentHost` doc) — so the catalogue
//!   every table below spans is read from `GetVersions` itself rather than written here.
//!
//! Both services are mounted on ONE shared engine, and every call goes over the wire a
//! client makes it on — awsJson1_0 for Amazon EVS, ec2Query for Amazon EC2 — so the
//! placement groups and Dedicated Hosts the accepted rows name are minted by Amazon EC2
//! itself, exactly as a caller would mint them.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{
    call, error_type, register_network, vlans_with_prefix, REGION, ZONE,
};

/// Long enough for the transitions the two services owe a new environment to come due
/// (each span is under a second by construction).
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The EC2 API version an ec2Query body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// The VLAN subnet prefix length every environment here is created with: a /24 leaves
/// room for every host a table adds.
const VLAN_PREFIX_LEN: u8 = 24;

/// What a row expects of `CreateEnvironmentHost`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The request is served: HTTP 200, and the host is stored.
    Accepted,
    /// The request is refused with a `ValidationException` whose `fieldList` carries an
    /// entry named by this member path.
    Refused(&'static str),
}

/// One emulator serving BOTH services off one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise behaviour, not signing.
        router.set_request_authentication(false);
        // An environment is created only for an account holding a Premium Support
        // subscription (`tests/all/premium_support_entitlement.rs` owns that rule).
        crate::premium_support_entitlement::grant_premium_support(&engine);
        router.register_service(
            service_com_amazonaws_evs_amazonelasticvmwareservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// A VPC and a subnet inside it in the account's own EC2 store, as
    /// `CreateEnvironment` requires — a fresh pair per environment, because Amazon EVS
    /// allows one deployed environment per VPC.
    fn fresh_network(&self) -> (String, String) {
        let ec2_state = ec2::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the account's EC2 store");
        let (vpc_id, subnet_id) = {
            let mut store = ec2_state.lock();
            (store.next_id("vpc"), store.next_id("subnet"))
        };
        register_network(&self.engine, &vpc_id, &subnet_id);
        (vpc_id, subnet_id)
    }

    /// Create an environment of `vcf_version` and wait for the deployment the clock owes
    /// it, so a host can be added to it. `case` names the case in every assertion.
    async fn deployed_environment(&self, case: &str, vcf_version: &str) -> String {
        let environment_name: String = case
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                    character
                } else {
                    '-'
                }
            })
            .collect();
        let (vpc_id, subnet_id) = self.fresh_network();
        let request = json!({
            "vpcId": vpc_id,
            "serviceAccessSubnetId": subnet_id,
            "vcfVersion": vcf_version,
            "termsAccepted": true,
            "environmentName": environment_name,
            "initialVlans": vlans_with_prefix(VLAN_PREFIX_LEN),
        });
        let (status, body) = call(&self.router, "CreateEnvironment", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEnvironment expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let environment = body["environment"]["environmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no environmentId in {body}"))
            .to_string();
        std::thread::sleep(PAST_THE_TRANSITION);
        environment
    }

    /// One ec2Query call: `application/x-www-form-urlencoded` in, XML out.
    async fn ec2_call(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
        for (name, value) in params {
            body.push_str(&format!("&{name}={value}"));
        }
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            // The `ec2` signing name in the credential scope is what routes the request
            // to Amazon EC2 and selects the ec2Query envelope.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/ec2/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0"
                ),
            )
            .body(::bytes::Bytes::from(body))
            .expect("a well-formed ec2Query request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// A placement group the account really holds, minted by Amazon EC2's own
    /// `CreatePlacementGroup`, and its `GroupId`.
    async fn placement_group(&self, group_name: &str, strategy: &str) -> String {
        let (status, body) = self
            .ec2_call(
                "CreatePlacementGroup",
                &[("GroupName", group_name), ("Strategy", strategy)],
            )
            .await;
        assert_eq!(
            status, 200,
            "CreatePlacementGroup {group_name:?}: expected 200, got {status} {body}"
        );
        elements(&body, "groupId")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("CreatePlacementGroup {group_name:?}: no groupId in {body}"))
    }

    /// A Dedicated Host the account really holds, allocated by Amazon EC2's own
    /// `AllocateHosts`, and its `HostId`.
    async fn dedicated_host(&self, instance_type: &str) -> String {
        let (status, body) = self
            .ec2_call(
                "AllocateHosts",
                &[
                    ("InstanceType", instance_type),
                    ("AvailabilityZone", ZONE),
                    ("Quantity", "1"),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "AllocateHosts {instance_type:?}: expected 200, got {status} {body}"
        );
        let allocated = elements(&body, "item");
        assert_eq!(
            allocated.len(),
            1,
            "AllocateHosts {instance_type:?}: expected one allocated host, got {allocated:?}"
        );
        allocated[0].clone()
    }

    /// Add one host, and answer the status and body `CreateEnvironmentHost` answered.
    async fn create_host(
        &self,
        environment: &str,
        host: &Value,
        esx_version: Option<&str>,
    ) -> (u16, Value) {
        let mut request = json!({ "environmentId": environment, "host": host });
        if let Some(esx_version) = esx_version {
            request["esxVersion"] = json!(esx_version);
        }
        call(&self.router, "CreateEnvironmentHost", &request).await
    }

    /// Every host of the environment, as `ListEnvironmentHosts` reports them.
    async fn hosts(&self, environment: &str) -> Vec<Value> {
        let (status, body) = call(
            &self.router,
            "ListEnvironmentHosts",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(status, 200, "ListEnvironmentHosts: got {status} {body}");
        body["environmentHosts"]
            .as_array()
            .unwrap_or_else(|| panic!("ListEnvironmentHosts: no environmentHosts in {body}"))
            .clone()
    }

    /// Every instance id the account's EC2 store reports.
    async fn instance_ids(&self) -> Vec<String> {
        let (status, body) = self.ec2_call("DescribeInstances", &[]).await;
        assert_eq!(
            status, 200,
            "DescribeInstances: expected 200, got {status} {body}"
        );
        let mut ids = elements(&body, "instanceId");
        ids.sort();
        ids
    }
}

/// The text of every `<name>…</name>` element of `xml`, in document order.
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

/// The `fieldList` member names a `ValidationException` body carries.
fn refused_members(body: &Value) -> Vec<String> {
    body["fieldList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A host, as a request names one: the members every row shares, plus the row's own.
fn host_request(host_name: &str, extra: &Value) -> Value {
    let mut host = json!({
        "hostName": host_name,
        "keyName": "evs-key",
        "instanceType": "i4i.metal",
    });
    for (member, value) in extra.as_object().expect("the row's members are an object") {
        host[member] = value.clone();
    }
    host
}

/// RULE: a host is placed only on EC2 resources the ACCOUNT's own Amazon EC2 store holds.
///
/// One row per form a placement reference comes in — no reference, a placement group of
/// each strategy the account holds, a Dedicated Host it holds, and ids of the short (8
/// hexadecimal digits) and long (17) forms the members' own patterns publish that nothing
/// holds — so the one id an issue cited is one row among many. A reference that resolves
/// is reported back by `ListEnvironmentHosts`; one that does not is refused with the
/// member named, and leaves the stores untouched.
#[tokio::test]
async fn a_host_is_placed_only_on_ec2_resources_the_account_holds() {
    let emulator = Emulator::new();
    let environment = emulator
        .deployed_environment("placement references", "VCF-5.2.1")
        .await;
    // References the account really holds, minted by Amazon EC2 itself.
    let cluster_group = emulator.placement_group("evs-cluster", "cluster").await;
    let partition_group = emulator.placement_group("evs-partition", "partition").await;
    let held_host = emulator.dedicated_host("i4i.metal").await;

    let rows: Vec<(&str, Value, Expected)> = vec![
        ("no placement reference at all", json!({}), Expected::Accepted),
        (
            "a cluster placement group the account holds",
            json!({ "placementGroupId": cluster_group }),
            Expected::Accepted,
        ),
        (
            "a partition placement group the account holds",
            json!({ "placementGroupId": partition_group }),
            Expected::Accepted,
        ),
        (
            "a Dedicated Host the account holds",
            json!({ "dedicatedHostId": held_host }),
            Expected::Accepted,
        ),
        (
            "a short-form placement group id nothing holds",
            json!({ "placementGroupId": "pg-00000000" }),
            Expected::Refused("host.placementGroupId"),
        ),
        (
            "a long-form placement group id nothing holds",
            json!({ "placementGroupId": "pg-0123456789abcdef0" }),
            Expected::Refused("host.placementGroupId"),
        ),
        (
            "a short-form Dedicated Host id nothing holds",
            json!({ "dedicatedHostId": "h-00000000" }),
            Expected::Refused("host.dedicatedHostId"),
        ),
        (
            "a long-form Dedicated Host id nothing holds",
            json!({ "dedicatedHostId": "h-0123456789abcdef0" }),
            Expected::Refused("host.dedicatedHostId"),
        ),
        (
            // The pair rule comes first: a host naming both mechanisms is refused for
            // that alone, so neither reference is resolved and the refusal names `host`.
            "a held placement group beside a Dedicated Host id nothing holds",
            json!({ "placementGroupId": cluster_group, "dedicatedHostId": "h-00000000" }),
            Expected::Refused("host"),
        ),
    ];

    for (index, (case, extra, expected)) in rows.iter().enumerate() {
        let host_name = format!("esx-{index}");
        let host = host_request(&host_name, extra);
        let instances_before = emulator.instance_ids().await;
        let hosts_before = emulator.hosts(&environment).await.len();
        let (status, body) = emulator.create_host(&environment, &host, None).await;

        match expected {
            Expected::Accepted => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
                    error_type(&body)
                );
                // The references the request named are reported back on the created host
                // and on every later read of it.
                let listed = emulator.hosts(&environment).await;
                assert_eq!(
                    listed.len(),
                    hosts_before + 1,
                    "case {case:?}: expected one more stored host, got {listed:?}"
                );
                let stored = listed
                    .iter()
                    .find(|stored| stored["hostName"] == json!(host_name))
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: {host_name} is not reported in {listed:?}")
                    });
                for member in ["placementGroupId", "dedicatedHostId"] {
                    let asked = host.get(member).cloned().unwrap_or(Value::Null);
                    assert_eq!(
                        body["host"][member], asked,
                        "case {case:?}: CreateEnvironmentHost {member}: expected {asked}, got {}",
                        body["host"][member]
                    );
                    assert_eq!(
                        stored[member], asked,
                        "case {case:?}: ListEnvironmentHosts {member}: expected {asked}, got {}",
                        stored[member]
                    );
                }
            }
            Expected::Refused(member) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
                    error_type(&body)
                );
                assert_eq!(
                    error_type(&body),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {body}"
                );
                let members = refused_members(&body);
                assert!(
                    members.iter().any(|name| name == member),
                    "case {case:?}: expected a fieldList entry for {member:?}, got \
                     {members:?} in {body}"
                );
                // A refused request stores no host and launches no instance.
                let listed = emulator.hosts(&environment).await;
                assert_eq!(
                    listed.len(),
                    hosts_before,
                    "case {case:?}: a refused request must store no host, got {listed:?}"
                );
                assert_eq!(
                    emulator.instance_ids().await,
                    instances_before,
                    "case {case:?}: a refused request must launch no instance"
                );
            }
        }
    }
}

/// RULE: the same placement references an environment's own `hosts` entries name are
/// resolved against the same store, and the refusal names the entry that named one.
///
/// `CreateEnvironment` takes four `HostInfoForCreate` entries, so the rule is asserted at
/// the path a caller wrote — one row per entry index, each a different form of reference.
#[tokio::test]
async fn an_environment_request_resolves_the_placement_references_of_its_own_hosts() {
    let emulator = Emulator::new();
    let held_group = emulator.placement_group("env-cluster", "cluster").await;
    let held_host = emulator.dedicated_host("i7i.metal-24xl").await;
    let (vpc_id, subnet_id) = emulator.fresh_network();

    // Entry 0 names a group the account holds, entries 1 and 2 name references nothing
    // holds, and entry 3 names none at all.
    let hosts = json!([
        { "hostName": "esx-0", "keyName": "evs-key", "instanceType": "i4i.metal", "placementGroupId": held_group },
        { "hostName": "esx-1", "keyName": "evs-key", "instanceType": "i4i.metal", "placementGroupId": "pg-00000000" },
        { "hostName": "esx-2", "keyName": "evs-key", "instanceType": "i4i.metal", "dedicatedHostId": "h-0123456789abcdef0" },
        { "hostName": "esx-3", "keyName": "evs-key", "instanceType": "i4i.metal" },
    ]);
    let request = json!({
        "vpcId": vpc_id,
        "serviceAccessSubnetId": subnet_id,
        "vcfVersion": "VCF-5.2.1",
        "termsAccepted": true,
        "environmentName": "host-placement",
        "initialVlans": vlans_with_prefix(VLAN_PREFIX_LEN),
        "hosts": hosts,
    });
    let (status, body) = call(&emulator.router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 400,
        "an environment whose hosts name phantom placement resources: expected HTTP 400, \
         got {status} ({}) {body}",
        error_type(&body)
    );
    let members = refused_members(&body);
    for member in ["hosts[1].placementGroupId", "hosts[2].dedicatedHostId"] {
        assert!(
            members.iter().any(|name| name == member),
            "expected a fieldList entry for {member:?}, got {members:?} in {body}"
        );
    }
    for member in ["hosts[0].placementGroupId", "hosts[3].placementGroupId"] {
        assert!(
            !members.iter().any(|name| name == member),
            "a reference the account holds must draw no clause: got {members:?} in {body}"
        );
    }

    // The same request with every reference resolvable is served, and the environment the
    // hosts belong to is created.
    let (vpc_id, subnet_id) = emulator.fresh_network();
    let mut accepted = request.clone();
    accepted["vpcId"] = json!(vpc_id);
    accepted["serviceAccessSubnetId"] = json!(subnet_id);
    accepted["environmentName"] = json!("host-placement-held");
    accepted["hosts"][1]["placementGroupId"] = json!(held_group);
    accepted["hosts"][2]["dedicatedHostId"] = json!(held_host);
    let (status, body) = call(&emulator.router, "CreateEnvironment", &accepted).await;
    assert_eq!(
        status, 200,
        "an environment whose hosts name held placement resources: expected HTTP 200, got \
         {status} ({}) {body}",
        error_type(&body)
    );
}

/// RULE: an `esxVersion` the request names is one of the versions Amazon EVS offers for
/// the environment's VCF version.
///
/// The catalogue is read from `GetVersions` — the action the operation's own doc names for
/// it — so every VCF version this service provides is a row, and the version each row
/// accepts is that catalogue's own `defaultEsxVersion` rather than a constant written
/// here. The refused rows differ in form from it: a higher release, a shorter version, a
/// word, and the offered version with a suffix.
#[tokio::test]
async fn an_esx_version_must_be_one_the_catalogue_offers_for_the_vcf_version() {
    let emulator = Emulator::new();
    let (status, body) = call(&emulator.router, "GetVersions", &json!({})).await;
    assert_eq!(
        status, 200,
        "GetVersions: expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let catalogue: Vec<(String, String)> = body["vcfVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("GetVersions: no vcfVersions in {body}"))
        .iter()
        .map(|version| {
            (
                version["vcfVersion"]
                    .as_str()
                    .unwrap_or_else(|| panic!("GetVersions: no vcfVersion in {version}"))
                    .to_string(),
                version["defaultEsxVersion"]
                    .as_str()
                    .unwrap_or_else(|| panic!("GetVersions: no defaultEsxVersion in {version}"))
                    .to_string(),
            )
        })
        .collect();
    assert!(
        !catalogue.is_empty(),
        "GetVersions must report the VCF versions Amazon EVS provides, got {body}"
    );

    for (vcf_version, offered) in &catalogue {
        let environment = emulator.deployed_environment(vcf_version, vcf_version).await;
        // Host DNS hostnames are unique ACROSS environments, so each row's name carries
        // the VCF version its environment was created with.
        let slug = vcf_version.replace('.', "-");
        // Accepted: no version at all (the default is resolved for the caller), and the
        // version the catalogue offers for this VCF version.
        let accepted: Vec<(String, Option<&str>)> = vec![
            ("no esxVersion at all".to_string(), None),
            (
                format!("the offered version {offered}"),
                Some(offered.as_str()),
            ),
        ];
        for (index, (case, esx_version)) in accepted.iter().enumerate() {
            let host_name = format!("esx-ok-{index}-{slug}");
            let (status, body) = emulator
                .create_host(&environment, &host_request(&host_name, &json!({})), *esx_version)
                .await;
            assert_eq!(
                status, 200,
                "case {vcf_version:?}/{case:?}: expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            let listed = emulator.hosts(&environment).await;
            assert!(
                listed
                    .iter()
                    .any(|stored| stored["hostName"] == json!(host_name)),
                "case {vcf_version:?}/{case:?}: {host_name} must be stored, got {listed:?}"
            );
        }

        // Refused: a version the catalogue does not offer for this VCF version, in each
        // form one can take. Every one is inside the modelled `@length(max: 128)`, so the
        // refusal is the operation's own rule and not the boundary validator's.
        let refused = [
            "9.9.9".to_string(),
            "8.0".to_string(),
            "latest".to_string(),
            format!("{offered}-1"),
        ];
        for (index, esx_version) in refused.iter().enumerate() {
            let stored_before = emulator.hosts(&environment).await.len();
            let host_name = format!("esx-bad-{index}-{slug}");
            let (status, body) = emulator
                .create_host(
                    &environment,
                    &host_request(&host_name, &json!({})),
                    Some(esx_version),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {vcf_version:?}/{esx_version:?}: expected HTTP 400, got {status} ({}) \
                 {body}",
                error_type(&body)
            );
            assert_eq!(
                error_type(&body),
                "ValidationException",
                "case {vcf_version:?}/{esx_version:?}: expected a ValidationException, got {body}"
            );
            let members = refused_members(&body);
            assert!(
                members.iter().any(|name| name == "esxVersion"),
                "case {vcf_version:?}/{esx_version:?}: expected a fieldList entry for \
                 \"esxVersion\", got {members:?} in {body}"
            );
            assert_eq!(
                emulator.hosts(&environment).await.len(),
                stored_before,
                "case {vcf_version:?}/{esx_version:?}: a refused request must store no host"
            );
        }
    }
}

/// RULE (the other side of it): a VCF version the catalogue states no ESX versions for
/// leaves nothing to judge a requested version against, so none is refused for it.
///
/// `GetVersions` reports the versions Amazon EVS PROVIDES, and `SELF_DEPLOYED` — "You
/// install VCF yourself" — is not one of them, so this service publishes no ESX versions
/// for it and cannot refuse one.
#[tokio::test]
async fn an_esx_version_is_not_judged_for_a_vcf_version_the_catalogue_states_none_for() {
    let emulator = Emulator::new();
    let (status, body) = call(&emulator.router, "GetVersions", &json!({})).await;
    assert_eq!(status, 200, "GetVersions: got {status} {body}");
    let provided: Vec<String> = body["vcfVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("GetVersions: no vcfVersions in {body}"))
        .iter()
        .filter_map(|version| version["vcfVersion"].as_str().map(str::to_string))
        .collect();
    assert!(
        !provided.iter().any(|version| version == "SELF_DEPLOYED"),
        "SELF_DEPLOYED names no version Amazon EVS provides, got {provided:?}"
    );

    let environment = emulator
        .deployed_environment("self-deployed", "SELF_DEPLOYED")
        .await;
    for (index, esx_version) in ["9.9.9", "8.0.3"].iter().enumerate() {
        let host_name = format!("esx-self-{index}");
        let (status, body) = emulator
            .create_host(
                &environment,
                &host_request(&host_name, &json!({})),
                Some(esx_version),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {esx_version:?}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
    }
}
