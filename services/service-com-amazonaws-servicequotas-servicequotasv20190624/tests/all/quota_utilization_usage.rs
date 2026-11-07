// SPDX-License-Identifier: CC0-1.0

//! What a quota utilization report says the account is USING, end to end over the wire.
//!
//! The rule under test: a record's `Utilization` is the account's CURRENT USAGE of that
//! quota over the value in force, times 100 — "calculated as (current usage / applied
//! value) x 100. Values range from 0.0 to 100.0 or higher if usage exceeds the quota
//! limit" (`QuotaUtilizationInfo$Utilization`) — where the usage is counted in the
//! resources the quota's own CloudWatch usage metric meters, read from the service that
//! owns them. So the figure moves with what the account holds: it is not a constant, a
//! quota whose usage this emulator holds no source for reports no `Utilization` at all
//! rather than a canned percentage, and the documented "highest utilization first"
//! ordering really does put the account's hottest quota on the first page.
//!
//! That rule spans two services, so both are mounted on ONE engine: Service Quotas is
//! driven over the wire the way a client drives it, and EC2 — which owns the VPCs and
//! the instances whose vCPUs the quotas are counted in — through the AWS SDK over the
//! same in-process callback the report's own measurement uses.
//!
//! The rows span the class the rule generalises over: no resources, one, several, a
//! count at the quota's limit and a count ABOVE it, two different quotas (a resource
//! count and a vCPU count), two purchase models of one vCPU group, a group whose
//! value in force is zero, a group running more instance types than one read of the
//! facts behind them can carry, and an account whose instances cannot be read at all.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_sdk_ec2::types::InstanceType;
use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The routing header this service is reached by: `POST /` with `X-Amz-Target:
/// ServiceQuotasV20190624.<OperationName>`.
const TARGET_PREFIX: &str = "ServiceQuotasV20190624.";

/// Comfortably past `crate::state::REPORT_COMPLETION_DELAY` (500 ms), so a report
/// started before this wait has completed afterwards.
const PAST_COMPLETION: Duration = Duration::from_millis(1_200);

/// How long a launch is given to reach `running` — the EC2 emulator moves an instance
/// out of `pending` on its own clock, a fraction of a second after the launch.
const LAUNCH_BUDGET: Duration = Duration::from_secs(10);

/// An AMI id a launch names; nothing here asserts anything about the image.
const AMI: &str = "ami-00112233445566778";

/// The `ServiceCode` of the quotas Amazon VPC publishes.
const VPC_SERVICE: &str = "vpc";
/// `VPCs per Region` — AWS's own `QuotaCode` for it, and the quota whose usage metric
/// counts the account's VPCs.
const VPCS_PER_REGION: &str = "L-F678F1CE";

/// The `ServiceCode` of the quotas Amazon EC2 publishes.
const EC2_SERVICE: &str = "ec2";
/// `Running On-Demand Standard (A, C, D, H, I, M, R, T, Z) instances` — AWS's own
/// `QuotaCode`, and the quota the vCPUs of a running On-Demand instance of the Standard
/// families are counted in.
const STANDARD_ON_DEMAND_VCPUS: &str = "L-1216C47A";
/// `All Standard (A, C, D, H, I, M, R, T, Z) Spot Instance Requests` — AWS's own
/// `QuotaCode` for the same group's SPOT vCPUs.
const STANDARD_SPOT_VCPUS: &str = "L-34B43A08";
/// The `QuotaName` of the X instance family's On-Demand vCPU quota, whose AWS default
/// value is zero.
const X_ON_DEMAND_QUOTA_NAME: &str = "Running On-Demand X instances";

/// Service Quotas and EC2 over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    fn new() -> Self {
        Self::build(true)
    }

    /// Service Quotas with NOTHING mounted that owns the resources its quotas are
    /// counted in: no usage source for the account's instances can be reached, so no
    /// count of them can be made.
    fn without_ec2() -> Self {
        Self::build(false)
    }

    fn build(with_ec2: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These callers sign nothing, so they must not be refused for it.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_servicequotas_servicequotasv20190624::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        if with_ec2 {
            router.register_service(
                service_com_amazonaws_ec2_amazonec2::EmulatedService::new(Arc::clone(&engine)),
            );
        }
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// One Service Quotas call for `region`, required to answer 200.
    async fn quotas(&self, region: &str, operation: &str, body: Value) -> Value {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("servicequotas.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            // The credential scope is what routes the request to this service and names
            // the Region it is addressed to.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTCLIENT/20260101/{region}/\
                     servicequotas/aws4_request, SignedHeaders=host;x-amz-date, \
                     Signature=notchecked"
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case {operation} in {region}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// An EC2 client for `region` over the in-process callback — the same seam the
    /// report's own usage measurement reaches EC2 through.
    fn ec2(&self, region: &str) -> aws_sdk_ec2::Client {
        let config = aws_sdk_ec2::Config::builder()
            .behavior_version(aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(aws_sdk_ec2::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_ec2::Client::from_conf(config)
    }
}

/// Start a report in `region`, wait for the clock to complete it, and answer every
/// record it publishes.
async fn report_records(emulator: &Emulator, region: &str) -> Vec<Value> {
    let started = emulator
        .quotas(region, "StartQuotaUtilizationReport", json!({}))
        .await;
    let report_id = started["ReportId"]
        .as_str()
        .unwrap_or_else(|| panic!("case a start in {region}: expected a ReportId, got {started}"))
        .to_string();
    tokio::time::sleep(PAST_COMPLETION).await;

    let mut records = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..512 {
        let mut body = json!({"ReportId": report_id});
        if let Some(next) = &token {
            body["NextToken"] = json!(next);
        }
        let page = emulator
            .quotas(region, "GetQuotaUtilizationReport", body)
            .await;
        assert_eq!(
            page["Status"],
            json!("COMPLETED"),
            "case the report in {region}: it must have completed, got {page}"
        );
        records.extend(
            page["Quotas"]
                .as_array()
                .unwrap_or_else(|| panic!("case the report in {region}: expected Quotas: {page}"))
                .clone(),
        );
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return records;
        }
    }
    panic!("case the report in {region}: the walk must terminate");
}

/// The record a report holds for one `(ServiceCode, QuotaCode)` pair.
fn record_for<'a>(records: &'a [Value], service_code: &str, quota_code: &str) -> &'a Value {
    records
        .iter()
        .find(|record| {
            record["ServiceCode"] == json!(service_code) && record["QuotaCode"] == json!(quota_code)
        })
        .unwrap_or_else(|| {
            panic!("the report must hold a record for {service_code}/{quota_code}")
        })
}

/// The record a report holds for one `(ServiceCode, QuotaName)` pair — how a quota AWS
/// publishes no `QuotaCode` of its own for is named.
fn record_named<'a>(records: &'a [Value], service_code: &str, quota_name: &str) -> &'a Value {
    records
        .iter()
        .find(|record| {
            record["ServiceCode"] == json!(service_code) && record["QuotaName"] == json!(quota_name)
        })
        .unwrap_or_else(|| panic!("the report must hold a record for {service_code}/{quota_name}"))
}

/// The `AppliedValue` of a record, which is the denominator of its utilization.
fn applied_value(record: &Value, case: &str) -> f64 {
    record["AppliedValue"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: a record reports an AppliedValue, got {record}"))
}

/// The `Utilization` of a record, which must be present.
fn utilization(record: &Value, case: &str) -> f64 {
    record["Utilization"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a Utilization, got {record}"))
}

/// How many VPCs EC2 holds for `region`, walked to the end of the listing.
async fn count_vpcs(emulator: &Emulator, region: &str) -> usize {
    let ec2 = emulator.ec2(region);
    let mut vpcs = 0;
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let page = ec2
            .describe_vpcs()
            .set_next_token(token.clone())
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: EC2 must describe its VPCs: {error:?}"));
        vpcs += page.vpcs().len();
        token = page.next_token().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    vpcs
}

/// Create `count` VPCs in `region` through EC2 itself.
async fn create_vpcs(emulator: &Emulator, region: &str, count: usize, cidr_prefix: u8) {
    let ec2 = emulator.ec2(region);
    for index in 0..count {
        let cidr = format!("10.{}.{index}.0/24", cidr_prefix);
        ec2.create_vpc()
            .cidr_block(&cidr)
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: EC2 must create the VPC {cidr}: {error:?}"));
    }
}

/// Launch `count` instances of `instance_type` in `region` and wait for every instance
/// of the account to be running, which is the state the vCPU quotas count.
async fn launch_running(emulator: &Emulator, region: &str, instance_type: &str, count: i32) {
    launch(emulator, region, instance_type, count).await;
    wait_until_all_running(emulator, region).await;
}

/// Launch `count` instances of `instance_type` in `region`, without waiting for them —
/// so a set of launches can be started before anything waits on the clock.
async fn launch(emulator: &Emulator, region: &str, instance_type: &str, count: i32) {
    let ec2 = emulator.ec2(region);
    ec2.run_instances()
        .image_id(AMI)
        .instance_type(InstanceType::from(instance_type))
        .min_count(count)
        .max_count(count)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("setup: EC2 must launch {count} x {instance_type}: {error:?}")
        });
}

/// Wait for every instance of the account to be running, which is the state the vCPU
/// quotas count.
async fn wait_until_all_running(emulator: &Emulator, region: &str) {
    let ec2 = emulator.ec2(region);
    let deadline = Instant::now() + LAUNCH_BUDGET;
    loop {
        let described = ec2
            .describe_instances()
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: EC2 must describe its instances: {error:?}"));
        let states: Vec<String> = described
            .reservations()
            .iter()
            .flat_map(|reservation| reservation.instances())
            .filter_map(|instance| instance.state().and_then(|state| state.name()))
            .map(|name| name.as_str().to_string())
            .collect();
        if states.iter().all(|state| state == "running") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the launched instances must reach running, their states are {states:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// How many vCPUs EC2 publishes for each of `types` (`VCpuInfo$DefaultVCpus`) — read
/// from EC2, which owns the fact, rather than written down here.
async fn vcpus_of(emulator: &Emulator, region: &str, types: &[&str]) -> BTreeMap<String, f64> {
    let ec2 = emulator.ec2(region);
    let described = ec2
        .describe_instance_types()
        .set_instance_types(Some(
            types.iter().map(|name| InstanceType::from(*name)).collect(),
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("setup: EC2 must describe its instance types: {error:?}"));
    let mut vcpus = BTreeMap::new();
    for info in described.instance_types() {
        let Some(name) = info.instance_type() else {
            continue;
        };
        let Some(default_vcpus) = info.v_cpu_info().and_then(|cpus| cpus.default_v_cpus()) else {
            continue;
        };
        vcpus.insert(name.as_str().to_string(), f64::from(default_vcpus));
    }
    for name in types {
        assert!(
            vcpus.contains_key(*name),
            "setup: EC2 must publish the vCPUs of {name}, got {vcpus:?}"
        );
    }
    vcpus
}

/// A resource-counted quota's utilization is the account's count of that resource over
/// the value in force — so it moves with every resource the account creates, reaches
/// 100 % at the limit and goes above it when the account holds more than the quota
/// permits.
#[tokio::test]
async fn a_resource_count_quota_reports_the_accounts_own_count_over_the_value_in_force() {
    let emulator = Emulator::new();

    // Rows: each a Region of its own, creating a further batch of VPCs — none, one and
    // several more each time — so the reported utilization is read at a higher count each
    // time. EC2 answers `DescribeVpcs` for the REGION the report is read in, so each row's
    // count is that Region's own: the default VPC it comes with plus what the row created
    // there. Each row reads the count EC2 itself reports rather than assuming one.
    let rows: &[(&str, usize, u8)] = &[
        ("us-east-1", 0, 10),
        ("us-west-2", 1, 20),
        ("eu-west-1", 3, 30),
        ("ap-south-1", 5, 40),
    ];

    let mut previous_utilization = -1.0_f64;
    let mut highest_region = String::new();
    for (region, new_vpcs, cidr_prefix) in rows {
        let case = format!("{new_vpcs} further VPCs, reported in {region}");
        // Reading the count first is also what materialises the Region's own default
        // VPC ("we create a default VPC for you in each AWS Region"), so the count the
        // report measures is the one asserted against.
        let before = count_vpcs(&emulator, region).await;
        create_vpcs(&emulator, region, *new_vpcs, *cidr_prefix).await;
        let held = count_vpcs(&emulator, region).await;
        assert_eq!(
            held,
            before + new_vpcs,
            "case {case}: the account must hold the VPCs it created, got {held}"
        );

        let records = report_records(&emulator, region).await;
        let record = record_for(&records, VPC_SERVICE, VPCS_PER_REGION);
        let applied = applied_value(record, &case);
        let reported = utilization(record, &case);
        #[allow(clippy::cast_precision_loss)]
        let expected = held as f64 / applied * 100.0;
        assert_eq!(
            reported, expected,
            "case {case}: expected the {held} VPCs over the applied value {applied} as \
             {expected}, got {reported} in {record}"
        );
        assert!(
            reported > previous_utilization,
            "case {case}: utilization must follow what the account holds, but it did not \
             rise above {previous_utilization}: got {reported}"
        );
        // The usage metric's namespace is what the record reports the usage was tracked
        // through, and the AWS default is untouched by any of this.
        assert_eq!(
            record["Namespace"],
            json!("AWS/Usage"),
            "case {case}: expected the usage metric's namespace, got {record}"
        );
        previous_utilization = reported;
        highest_region = (*region).to_string();
    }

    // The last row holds more VPCs than the quota permits, which the documented range
    // allows for ("or higher if usage exceeds the quota limit") — and makes that quota
    // the account's hottest, so the documented "highest utilization first" ordering puts
    // it on the first page, alone when the page holds one record.
    assert!(
        previous_utilization > 100.0,
        "case the account past the VPC limit: expected a utilization above 100 %, got \
         {previous_utilization}"
    );
    let started = emulator
        .quotas(&highest_region, "StartQuotaUtilizationReport", json!({}))
        .await;
    let first_page = emulator
        .quotas(
            &highest_region,
            "GetQuotaUtilizationReport",
            json!({"ReportId": started["ReportId"], "MaxResults": 1}),
        )
        .await;
    let first = first_page["Quotas"]
        .as_array()
        .and_then(|records| records.first())
        .unwrap_or_else(|| panic!("case the first page: expected one record, got {first_page}"))
        .clone();
    assert_eq!(
        (first["ServiceCode"].clone(), first["QuotaCode"].clone()),
        (json!(VPC_SERVICE), json!(VPCS_PER_REGION)),
        "case the first page: expected the account's hottest quota first, got {first}"
    );
}

/// A vCPU quota's utilization is the vCPUs of the account's running instances of that
/// group and purchase model over the value in force: a group with nothing running in it
/// reads 0 %, the On-Demand and the Spot quota of one group are counted separately, and
/// an instance family belongs to its own group only.
#[tokio::test]
async fn a_vcpu_quota_reports_the_vcpus_of_the_accounts_running_instances() {
    let emulator = Emulator::new();
    let region = "us-east-2";

    // Rows: two instances of one Standard family and one of another, so the usage is a
    // SUM over instances rather than a count of them, and the two families cannot be
    // confused with each other.
    let launches: &[(&str, i32)] = &[("t3.micro", 2), ("m5.large", 1)];
    let vcpus = vcpus_of(
        &emulator,
        region,
        &launches.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
    )
    .await;
    for (instance_type, count) in launches {
        launch_running(&emulator, region, instance_type, *count).await;
    }
    let expected_vcpus: f64 = launches
        .iter()
        .map(|(instance_type, count)| vcpus[*instance_type] * f64::from(*count))
        .sum();

    let records = report_records(&emulator, region).await;

    let case = "the Standard group's On-Demand vCPUs";
    let on_demand = record_for(&records, EC2_SERVICE, STANDARD_ON_DEMAND_VCPUS);
    let applied = applied_value(on_demand, case);
    let expected = expected_vcpus / applied * 100.0;
    assert_eq!(
        utilization(on_demand, case),
        expected,
        "case {case}: expected the {expected_vcpus} running vCPUs over the applied value \
         {applied} as {expected}, got {on_demand}"
    );

    // Nothing was bought on the Spot market, so the same group's Spot quota is COUNTED
    // and reads zero — a measurement, not an absent one.
    let case = "the Standard group's Spot vCPUs with no Spot Instance running";
    assert_eq!(
        utilization(record_for(&records, EC2_SERVICE, STANDARD_SPOT_VCPUS), case),
        0.0,
        "case {case}: expected 0 %, got {:?}",
        record_for(&records, EC2_SERVICE, STANDARD_SPOT_VCPUS)
    );

    // A group with nothing running in it, whose value in force is zero: none of what it
    // permits is in use, so it reads 0 % rather than nothing at all.
    let case = "a vCPU group with a zero value in force and nothing running";
    let idle_group = record_named(&records, EC2_SERVICE, X_ON_DEMAND_QUOTA_NAME);
    assert_eq!(
        applied_value(idle_group, case),
        0.0,
        "case {case}: expected a zero value in force, got {idle_group}"
    );
    assert_eq!(
        utilization(idle_group, case),
        0.0,
        "case {case}: expected 0 %, got {idle_group}"
    );

    // An instance of ANOTHER group moves that group only — and with a value in force of
    // zero and usage above it there is no finite percentage to report, so that record
    // carries no `Utilization` at all rather than an invented number. Read in a SECOND
    // Region, because a start reuses the live report of its own Region — so that Region
    // holds the same Standard launches beside the X instance, EC2 counting the instances
    // of the Region the report is read in.
    let elsewhere = "ca-central-1";
    for (instance_type, count) in launches {
        launch_running(&emulator, elsewhere, instance_type, *count).await;
    }
    launch_running(&emulator, elsewhere, "x2iedn.xlarge", 1).await;
    let records = report_records(&emulator, elsewhere).await;
    let case = "the Standard group with an X instance running";
    let on_demand = record_for(&records, EC2_SERVICE, STANDARD_ON_DEMAND_VCPUS);
    assert_eq!(
        utilization(on_demand, case),
        expected,
        "case {case}: an instance of another group must not be counted here, got \
         {on_demand}"
    );
    let case = "a vCPU group with a zero value in force and an instance running";
    let x_group = record_named(&records, EC2_SERVICE, X_ON_DEMAND_QUOTA_NAME);
    assert!(
        x_group["Utilization"].is_null(),
        "case {case}: usage above a zero value in force has no finite percentage, got \
         {x_group}"
    );
}

/// A quota whose usage this emulator holds no source for reports no `Utilization` at
/// all: a percentage is reported exactly for the quotas that were counted, and never as
/// a stand-in for a count nobody made.
#[tokio::test]
async fn a_quota_with_no_usage_source_reports_no_utilization() {
    let emulator = Emulator::new();
    let records = report_records(&emulator, "eu-central-1").await;

    for record in &records {
        let case = format!(
            "{}/{}",
            record["ServiceCode"].as_str().unwrap_or_default(),
            record["QuotaCode"].as_str().unwrap_or_default()
        );
        // A quota publishes a usage metric or it does not; without one there is nothing
        // to count it by, so a `Utilization` can only ever accompany a `Namespace`.
        if record["Namespace"].is_null() {
            assert!(
                record["Utilization"].is_null(),
                "case {case}: a quota with no usage metric cannot report a utilization, \
                 got {record}"
            );
        }
    }

    // Lambda's concurrent executions are metered by a usage metric, and nothing in this
    // emulator holds how many invocations are in flight — so that quota is reported
    // without a utilization rather than with a canned zero.
    let case = "a usage metric with no source in this emulator";
    let concurrent_executions = record_for(&records, "lambda", "L-B99A9384");
    assert!(
        !concurrent_executions["Namespace"].is_null(),
        "case {case}: the quota must publish its usage metric, got {concurrent_executions}"
    );
    assert!(
        concurrent_executions["Utilization"].is_null(),
        "case {case}: expected no utilization, got {concurrent_executions}"
    );

    // And the quotas EC2 IS the source for carry one, counted at this moment, in an
    // account that holds only what every account holds.
    let case = "a counted quota in an account that created nothing";
    let vpcs = record_for(&records, VPC_SERVICE, VPCS_PER_REGION);
    let applied = applied_value(vpcs, case);
    let held = count_vpcs(&emulator, "eu-central-1").await;
    #[allow(clippy::cast_precision_loss)]
    let expected = held as f64 / applied * 100.0;
    assert_eq!(
        utilization(vpcs, case),
        expected,
        "case {case}: expected the {held} VPCs the account holds as {expected}, got {vpcs}"
    );
}

/// The instance families the Standard vCPU group meters, as its own published quota
/// name spells them out: "Running On-Demand Standard (A, C, D, H, I, M, R, T, Z)
/// instances" (and the same letters in `All Standard (A, C, D, H, I, M, R, T, Z) Spot
/// Instance Requests`). Which TYPES those families hold, and how many vCPUs each has,
/// is read from EC2 rather than written down here.
const STANDARD_FAMILY_LETTERS: &[&str] = &["a", "c", "d", "h", "i", "m", "r", "t", "z"];

/// The most instance types one `DescribeInstanceTypes` request may name: the modelled
/// `@length 0..100` on `DescribeInstanceTypesRequest$InstanceTypes`
/// (`smithy_query(query="shape",shape="RequestInstanceTypeList")`). An account running
/// more types than this cannot have its vCPUs read in one call.
const MAX_INSTANCE_TYPES_PER_CALL: usize = 100;

/// The first `count` instance types of the Standard vCPU group, in name order, each with
/// the vCPUs EC2 publishes for it (`VCpuInfo$DefaultVCpus`) — walked over EC2's own
/// pages, so the list is the catalog's and not one asserted here.
async fn standard_types(emulator: &Emulator, region: &str, count: usize) -> Vec<(String, f64)> {
    let ec2 = emulator.ec2(region);
    let mut types: Vec<(String, f64)> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let page = ec2
            .describe_instance_types()
            .max_results(100)
            .set_next_token(token.clone())
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("setup: EC2 must describe its instance types: {error:?}")
            });
        for info in page.instance_types() {
            let Some(name) = info.instance_type().map(|name| name.as_str().to_string()) else {
                continue;
            };
            let Some(vcpus) = info.v_cpu_info().and_then(|cpus| cpus.default_v_cpus()) else {
                continue;
            };
            // An instance type is `<family><generation and attributes>.<size>`, so the
            // family's own letters are the leading alphabetic run of the part before
            // the size.
            let family: String = name
                .split('.')
                .next()
                .unwrap_or_default()
                .chars()
                .take_while(char::is_ascii_alphabetic)
                .collect();
            if STANDARD_FAMILY_LETTERS.contains(&family.as_str()) {
                types.push((name, f64::from(vcpus)));
            }
        }
        token = page.next_token().map(str::to_string);
        if token.is_none() || types.len() >= count {
            break;
        }
    }
    types.truncate(count);
    assert_eq!(
        types.len(),
        count,
        "setup: EC2 must publish at least {count} Standard instance types, got {}",
        types.len()
    );
    types
}

/// A vCPU quota's utilization is the sum over EVERY running instance of its group,
/// however many calls and pages the facts behind them take to read: a
/// `DescribeInstanceTypes` request names at most 100 types and answers one page at a
/// time, so an account running more types than that still reports the whole sum —
/// never a partial one, and never the 0 % that would say the group is idle.
#[tokio::test]
async fn a_vcpu_quota_counts_every_running_type_however_many_reads_the_facts_take() {
    let emulator = Emulator::new();

    // Rows: how many DISTINCT instance types the account runs, growing past the most
    // one read of their vCPUs can carry — one, a few, and more than
    // `MAX_INSTANCE_TYPES_PER_CALL`, which takes a second call (and, where the answer
    // pages, a second page) before the group's count is complete.
    //
    // EC2 answers for the REGION the report is read in, so each row launches its whole set
    // of types in its own Region — which is also what a start of a report needs, since a
    // start reuses the live report of its Region.
    let rows: &[(&str, usize)] = &[
        ("us-west-1", 1),
        ("ap-northeast-1", 4),
        ("sa-east-1", MAX_INSTANCE_TYPES_PER_CALL + 4),
    ];
    let largest = rows
        .iter()
        .map(|(_, distinct)| *distinct)
        .max()
        .expect("the rows are not empty");
    let types = standard_types(&emulator, "us-west-1", largest).await;

    for (region, distinct) in rows {
        let case = format!("{distinct} distinct Standard instance types running");
        for (instance_type, _) in &types[..*distinct] {
            launch(&emulator, region, instance_type, 1).await;
        }
        wait_until_all_running(&emulator, region).await;

        let expected_vcpus: f64 = types[..*distinct].iter().map(|(_, vcpus)| vcpus).sum();
        let records = report_records(&emulator, region).await;
        let record = record_for(&records, EC2_SERVICE, STANDARD_ON_DEMAND_VCPUS);
        let applied = applied_value(record, &case);
        let expected = expected_vcpus / applied * 100.0;
        let reported = utilization(record, &case);
        assert_eq!(
            reported, expected,
            "case {case}: expected the {expected_vcpus} running vCPUs over the applied \
             value {applied} as {expected}, got {reported} in {record}"
        );
        assert!(
            reported > 0.0,
            "case {case}: a group with instances running must not read as idle, got \
             {reported} in {record}"
        );
    }
}

/// A count that could NOT be made is published as no `Utilization` at all rather than
/// as 0 %: zero is a measurement saying the account holds none of what the quota
/// permits, so it is reported exactly when the group really was counted.
#[tokio::test]
async fn a_vcpu_quota_reports_no_utilization_when_its_count_could_not_be_made() {
    // Rows: the same quotas, read in an account whose instances EC2 answers for and in
    // one where no source for them can be reached at all.
    let rows: &[(&str, bool)] = &[
        ("EC2 answers for the account's instances", true),
        ("no source for the account's instances", false),
    ];
    // Both purchase models of one group, and a group whose value in force is zero: a
    // reading covers a group whatever its limit, and the two models are counted apart.
    let quotas: &[&str] = &[STANDARD_ON_DEMAND_VCPUS, STANDARD_SPOT_VCPUS];

    for (row, counted) in rows {
        let emulator = if *counted {
            Emulator::new()
        } else {
            Emulator::without_ec2()
        };
        let records = report_records(&emulator, "eu-west-2").await;
        for quota_code in quotas {
            let case = format!("{quota_code} with {row}");
            let record = record_for(&records, EC2_SERVICE, quota_code);
            // Counted or not, the quota is still published with the usage metric it
            // meters; what changes is whether a reading was made for it.
            assert!(
                !record["Namespace"].is_null(),
                "case {case}: the quota must publish its usage metric, got {record}"
            );
            if *counted {
                assert_eq!(
                    utilization(record, &case),
                    0.0,
                    "case {case}: a counted group with nothing running reads 0 %, got \
                     {record}"
                );
            } else {
                assert!(
                    record["Utilization"].is_null(),
                    "case {case}: a count that could not be made is no utilization at \
                     all, got {record}"
                );
            }
        }

        let case = format!("{X_ON_DEMAND_QUOTA_NAME} with {row}");
        let idle_group = record_named(&records, EC2_SERVICE, X_ON_DEMAND_QUOTA_NAME);
        assert_eq!(
            idle_group["Utilization"].is_null(),
            !*counted,
            "case {case}: a utilization is published exactly when the group was \
             counted, got {idle_group}"
        );
    }
}
