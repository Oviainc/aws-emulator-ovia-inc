// SPDX-License-Identifier: CC0-1.0

//! What the account is USING of its quotas right now, read out of the services that own
//! the resources a quota is counted in.
//!
//! A quota's `Utilization` is "the utilization percentage of the quota, calculated as
//! (current usage / applied value) x 100" (`QuotaUtilizationInfo$Utilization`), so the
//! numerator has to be the account's CURRENT CONSUMPTION — not a number this service
//! keeps for itself. Service Quotas does not own any of the resources it reports on; it
//! tracks each quota through the CloudWatch usage metric the quota publishes
//! (`ServiceQuota$UsageMetric`), and that metric names exactly what is counted: the
//! `Service` the usage belongs to, the `Resource` counted, the `Class` of usage and
//! whether it is a resource count or a call count.
//!
//! So a reading here is keyed by that METRIC IDENTITY rather than by a quota code: a
//! reader counts the resources of one metric, every quota carrying that metric gets the
//! reading, and no second table has to agree with the catalog about which quota is
//! metered by which metric.
//!
//! The resources themselves are counted by asking the service that owns them, over the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]) the
//! framework gives a handler for calling another emulated service — the same seam the
//! load balancing service resolves the account's network through. A VPC is EC2's
//! resource and the vCPUs of a running instance are EC2's fact, so EC2 is asked for
//! both instead of this service keeping a count of its own that nothing updates.
//!
//! A metric no reader here can count is left with NO reading, and the quota carrying it
//! then reports no `Utilization` at all (see `crate::state::utilization_pct`): an
//! account's in-flight Lambda invocations or its API call rate are not state this
//! emulator holds, and a canned percentage for them would be a number no client could
//! act on. A reading of `0.0` therefore means "counted, and nothing is in use", which is
//! a different answer from "not counted".

use std::collections::BTreeMap;

use crate::ops::types::MetricInfo;

/// How many pages of one listing a measurement walks before it stops.
///
/// A measurement is made inside a request, so it is bounded rather than walked to the
/// end of whatever the account holds: EC2 answers up to 1,000 items per page, so this
/// covers hundreds of thousands of resources and still terminates if a listing ever
/// kept handing out tokens.
const MAX_LISTING_PAGES: usize = 64;

/// The most instance types one `DescribeInstanceTypes` call is asked about: the bound
/// the EC2 model declares on `DescribeInstanceTypesRequest$InstanceTypes` (`@length
/// 0..100`), so a batch is never refused for its size.
const MAX_INSTANCE_TYPES_PER_CALL: usize = 100;

/// The `Instance$State` name of an instance that is RUNNING — the state whose vCPUs the
/// On-Demand and Spot vCPU quotas are counted in ("Running On-Demand … instances").
const INSTANCE_STATE_RUNNING: &str = "running";

/// The `Instance$InstanceLifecycle` word EC2 reports for a Spot Instance; an On-Demand
/// instance carries none.
const INSTANCE_LIFECYCLE_SPOT: &str = "spot";

/// The identity of one CloudWatch usage metric: its namespace, its metric name and the
/// dimensions that pin down what is counted. Two metrics are the same usage source
/// exactly when all three agree, which is what makes a reading apply to every quota
/// publishing that metric.
type MetricIdentity = (String, String, BTreeMap<String, String>);

/// The account's measured usage, per usage metric.
///
/// A metric is in the map only when something actually counted it, so the absence of an
/// entry is the honest answer "this emulator holds no usage source for that metric"
/// rather than a usage of zero.
#[derive(Debug, Default, Clone)]
pub struct QuotaUsage {
    readings: BTreeMap<MetricIdentity, f64>,
}

impl QuotaUsage {
    /// Record `metric` as counted, reading `usage`.
    pub fn set(&mut self, metric: &MetricInfo, usage: f64) {
        if let Some(identity) = identity(metric) {
            self.readings.insert(identity, usage);
        }
    }

    /// Add `usage` to a metric already counted — how a per-resource count accumulates
    /// into the one figure its metric reports. A metric no [`QuotaUsage::set`] declared
    /// counted is NOT created here: a count only ever accumulates into a reading whose
    /// reader established that it could be measured at all.
    pub fn add(&mut self, metric: &MetricInfo, usage: f64) {
        if let Some(identity) = identity(metric) {
            if let Some(reading) = self.readings.get_mut(&identity) {
                *reading += usage;
            }
        }
    }

    /// The account's usage of the quota `metric` meters, or `None` when nothing counted
    /// that metric.
    pub fn reading(&self, metric: &MetricInfo) -> Option<f64> {
        self.readings.get(&identity(metric)?).copied()
    }

    /// How many usage metrics this reading covers. Zero means no usage source could be
    /// reached at all, so no record will carry a `Utilization`.
    pub fn measured(&self) -> usize {
        self.readings.len()
    }
}

/// The identity of `metric`, or `None` for a metric that names no namespace or no
/// metric name — neither of which identifies a usage source.
fn identity(metric: &MetricInfo) -> Option<MetricIdentity> {
    let namespace = metric.metric_namespace.clone()?;
    let name = metric.metric_name.clone()?;
    let dimensions = metric
        .metric_dimensions
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();
    Some((namespace, name, dimensions))
}

/// Measure the account's usage of every quota this emulator holds a usage source for,
/// as of now, in `region`.
///
/// Called when a quota utilization report is INITIATED: the report freezes what it
/// measured, so every page of one report reports the same figures.
pub async fn measure(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> QuotaUsage {
    let mut usage = QuotaUsage::default();
    measure_ec2(ctx, region, &mut usage).await;
    usage
}

/// An EC2 client over the in-process callback, addressed to `region`.
///
/// Every request this client makes is served by the same router that is serving the
/// call being handled: the callback IS the transport, so nothing here opens a
/// connection, whatever the endpoint URL spells. The Region in that URL comes from the
/// caller's own credential scope, already narrowed to a Region this emulator serves
/// (`crate::state::request_region`), so the address names a service of this emulator and
/// nothing a caller chose.
///
/// The transport being in process is also why there is nothing to retry, time out or
/// watch for a stalled stream, and why the credentials the callback hands over are used
/// directly.
fn ec2_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// The usage EC2 holds the source for: the account's VPCs, and the vCPUs of its running
/// instances.
async fn measure_ec2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    usage: &mut QuotaUsage,
) {
    let ec2 = ec2_client(ctx, region);
    measure_vpcs(&ec2, usage).await;
    measure_running_vcpus(&ec2, usage).await;
}

/// The account's VPC count, as EC2 reports it for this Region.
async fn measure_vpcs(ec2: &::aws_sdk_ec2::Client, usage: &mut QuotaUsage) {
    let mut vpcs = 0usize;
    let mut next_token: Option<String> = None;
    for _ in 0..MAX_LISTING_PAGES {
        let answer = ec2
            .describe_vpcs()
            .set_next_token(next_token.clone())
            .send()
            .await;
        let page = match answer {
            Ok(page) => page,
            Err(error) => {
                ::tracing::debug!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the account's VPCs could not be counted against EC2"
                );
                return;
            }
        };
        vpcs += page.vpcs().len();
        next_token = page.next_token().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    usage.set(&super::vpcs_per_region_usage_metric(), vpcs as f64);
}

/// One running instance, as the vCPU quotas count it.
struct RunningInstance {
    /// Its `InstanceType`, which is what its vCPU count is read from.
    instance_type: String,
    /// Whether it is a Spot Instance: `Instance$InstanceLifecycle` is "spot" for one a
    /// Spot Instance request launched, and absent for an On-Demand instance.
    spot: bool,
}

/// The vCPUs of the account's running instances, per vCPU quota group and purchase
/// model.
///
/// Both halves come from EC2: which instances are running and what they are, and how
/// many vCPUs an instance type has (`DescribeInstanceTypes`, `VCpuInfo$DefaultVCpus`).
/// Nothing is recorded unless BOTH reads answered — a count taken from half the facts
/// would report a usage of zero for a group that has instances running.
async fn measure_running_vcpus(ec2: &::aws_sdk_ec2::Client, usage: &mut QuotaUsage) {
    let Some(running) = running_instances(ec2).await else {
        return;
    };
    let mut types: Vec<String> = running
        .iter()
        .map(|instance| instance.instance_type.clone())
        .collect();
    types.sort();
    types.dedup();
    let Some(vcpus) = instance_type_vcpus(ec2, &types).await else {
        return;
    };

    // EC2 answered, so every vCPU group IS counted — a group with no instance running
    // in it reads zero, which is a measurement and not a placeholder.
    for class in super::vcpu_group_classes() {
        for purchase_model in [
            super::PURCHASE_MODEL_ON_DEMAND,
            super::PURCHASE_MODEL_SPOT,
        ] {
            usage.set(&super::vcpu_usage_metric_for(class, purchase_model), 0.0);
        }
    }

    for instance in &running {
        let Some(class) = vcpu_group_class(&instance.instance_type) else {
            continue;
        };
        let Some(instance_vcpus) = vcpus.get(&instance.instance_type) else {
            continue;
        };
        let purchase_model = if instance.spot {
            super::PURCHASE_MODEL_SPOT
        } else {
            super::PURCHASE_MODEL_ON_DEMAND
        };
        usage.add(
            &super::vcpu_usage_metric_for(class, purchase_model),
            f64::from(*instance_vcpus),
        );
    }
}

/// The account's running instances, or `None` when EC2 could not be asked.
async fn running_instances(ec2: &::aws_sdk_ec2::Client) -> Option<Vec<RunningInstance>> {
    let mut running = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..MAX_LISTING_PAGES {
        let answer = ec2
            .describe_instances()
            .set_next_token(next_token.clone())
            .send()
            .await;
        let page = match answer {
            Ok(page) => page,
            Err(error) => {
                ::tracing::debug!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the account's running instances could not be counted against EC2"
                );
                return None;
            }
        };
        for reservation in page.reservations() {
            for instance in reservation.instances() {
                let state_name = instance
                    .state()
                    .and_then(|state| state.name())
                    .map(|name| name.as_str().to_string());
                if state_name.as_deref() != Some(INSTANCE_STATE_RUNNING) {
                    continue;
                }
                let Some(instance_type) = instance.instance_type() else {
                    continue;
                };
                running.push(RunningInstance {
                    instance_type: instance_type.as_str().to_string(),
                    spot: instance.instance_lifecycle().map(|lifecycle| lifecycle.as_str())
                        == Some(INSTANCE_LIFECYCLE_SPOT),
                });
            }
        }
        next_token = page.next_token().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    Some(running)
}

/// How many vCPUs each of `types` has, as EC2 publishes it, or `None` when EC2 could
/// not be asked.
async fn instance_type_vcpus(
    ec2: &::aws_sdk_ec2::Client,
    types: &[String],
) -> Option<BTreeMap<String, i32>> {
    let mut vcpus = BTreeMap::new();
    for batch in types.chunks(MAX_INSTANCE_TYPES_PER_CALL) {
        let named: Vec<::aws_sdk_ec2::types::InstanceType> = batch
            .iter()
            .map(|instance_type| ::aws_sdk_ec2::types::InstanceType::from(instance_type.as_str()))
            .collect();
        let answer = ec2
            .describe_instance_types()
            .set_instance_types(Some(named))
            .send()
            .await;
        let described = match answer {
            Ok(described) => described,
            Err(error) => {
                ::tracing::debug!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the vCPUs of the account's instance types could not be read from EC2"
                );
                return None;
            }
        };
        for info in described.instance_types() {
            let Some(instance_type) = info.instance_type() else {
                continue;
            };
            let Some(default_vcpus) = info
                .v_cpu_info()
                .and_then(::aws_sdk_ec2::types::VCpuInfo::default_v_cpus)
            else {
                continue;
            };
            vcpus.insert(instance_type.as_str().to_string(), default_vcpus);
        }
    }
    Some(vcpus)
}

/// The instance-family token each vCPU quota group covers, as the published quota names
/// spell the groups out.
///
/// Source: the group names of the vCPU quotas this service's catalog seeds (AWS's
/// published On-Demand Instance and Spot Instance Request quotas) — "Standard (A, C, D,
/// H, I, M, R, T, Z)", "DL", "F", "G and VT", "High Memory", "Inf", "P", "Trn" and "X".
/// The High Memory group is the `u`-prefixed families, and the I family's
/// storage-optimized variants (`im4gn`, `is4gen`) are part of Standard.
///
/// A family token no entry names belongs to no group here, so its instances are counted
/// against no vCPU quota: an EC2 Mac instance (`mac1`, `mac2`) is metered as a Dedicated
/// Host instead, and a family AWS meters under a quota this catalog does not hold yet
/// has no quota for a count to go to.
const VCPU_GROUP_FAMILIES: &[(&str, &str)] = &[
    ("a", "Standard"),
    ("c", "Standard"),
    ("d", "Standard"),
    ("h", "Standard"),
    ("i", "Standard"),
    ("im", "Standard"),
    ("is", "Standard"),
    ("m", "Standard"),
    ("r", "Standard"),
    ("t", "Standard"),
    ("z", "Standard"),
    ("dl", "DL"),
    ("f", "F"),
    ("g", "G"),
    ("vt", "G"),
    ("u", "HighMemory"),
    ("inf", "Inf"),
    ("p", "P"),
    ("trn", "Trn"),
    ("x", "X"),
];

/// The vCPU quota group `instance_type` is metered in — the `Class` dimension token of
/// its usage metric — or `None` for a family no vCPU quota of this catalog meters.
fn vcpu_group_class(instance_type: &str) -> Option<&'static str> {
    // An instance type is `<family><generation and attributes>.<size>`, so the family's
    // own letters are the leading alphabetic run of the part before the size.
    let family = instance_type.split('.').next().unwrap_or(instance_type);
    let token: String = family
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    VCPU_GROUP_FAMILIES
        .iter()
        .find(|(prefix, _)| *prefix == token.as_str())
        .map(|(_, class)| *class)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The group an instance type is metered in is read off its FAMILY, so every size
    /// and generation of one family lands in the same group, and a family no vCPU quota
    /// covers lands in none.
    #[test]
    fn instance_families_map_onto_their_published_vcpu_group() {
        let rows: &[(&str, Option<&str>)] = &[
            ("t3.micro", Some("Standard")),
            ("t4g.nano", Some("Standard")),
            ("m5.large", Some("Standard")),
            ("m7i-flex.xlarge", Some("Standard")),
            ("c6gn.16xlarge", Some("Standard")),
            ("i4i.metal", Some("Standard")),
            ("im4gn.large", Some("Standard")),
            ("is4gen.medium", Some("Standard")),
            ("z1d.large", Some("Standard")),
            ("dl1.24xlarge", Some("DL")),
            ("f1.2xlarge", Some("F")),
            ("g5.xlarge", Some("G")),
            ("vt1.3xlarge", Some("G")),
            ("u-6tb1.metal", Some("HighMemory")),
            ("u7i-12tb.224xlarge", Some("HighMemory")),
            ("inf2.xlarge", Some("Inf")),
            ("p5.48xlarge", Some("P")),
            ("trn1.32xlarge", Some("Trn")),
            ("x2iedn.xlarge", Some("X")),
            ("mac1.metal", None),
            ("hpc7a.96xlarge", None),
            ("", None),
        ];
        for (instance_type, expected) in rows {
            let class = vcpu_group_class(instance_type);
            assert_eq!(
                class, *expected,
                "case {instance_type:?}: expected group {expected:?}, got {class:?}"
            );
        }
    }

    /// A reading is keyed by the metric's identity, so the catalog's own metric reads
    /// back what a reader recorded for it — and a metric nothing counted reads back
    /// nothing, which is what keeps an uncounted quota from reporting a percentage.
    #[test]
    fn a_reading_is_found_by_the_metric_that_was_counted() {
        let mut usage = QuotaUsage::default();
        assert_eq!(
            usage.reading(&super::super::vpcs_per_region_usage_metric()),
            None,
            "case nothing counted: expected no reading, got {:?}",
            usage.reading(&super::super::vpcs_per_region_usage_metric())
        );

        usage.set(&super::super::vpcs_per_region_usage_metric(), 4.0);
        usage.add(&super::super::vpcs_per_region_usage_metric(), 1.0);
        assert_eq!(
            usage.reading(&super::super::vpcs_per_region_usage_metric()),
            Some(5.0),
            "case counted then accumulated: expected 5 VPCs, got {:?}",
            usage.reading(&super::super::vpcs_per_region_usage_metric())
        );

        // A metric nothing declared counted does not spring into existence from an
        // accumulation.
        let standard_spot = super::super::vcpu_usage_metric_for(
            "Standard",
            super::super::PURCHASE_MODEL_SPOT,
        );
        usage.add(&standard_spot, 8.0);
        assert_eq!(
            usage.reading(&standard_spot),
            None,
            "case accumulated without being counted: expected no reading, got {:?}",
            usage.reading(&standard_spot)
        );
    }
}
