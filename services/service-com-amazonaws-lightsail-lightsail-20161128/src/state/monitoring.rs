// SPDX-License-Identifier: CC0-1.0

//! What this service watches and who it tells: the metrics each resource type
//! publishes, the alarms set on them, and the contact methods a notification is
//! sent to.

use super::*;

/// Every metric an instance publishes, each with the `unit` its data is
/// published in.
///
/// The list is the modelled `GetInstanceMetricDataRequest$metricName`
/// documentation — "Valid instance metric names are listed below, along with the
/// most useful `statistics` to include in your request, and the published `unit`
/// value" — and the `unit` of each row is the "published unit" that entry states.
/// Source:
/// `smithy_query(query="doc",shape="GetInstanceMetricDataRequest",member="metricName")`.
/// It covers every member of the modelled `InstanceMetricName` enum, which is
/// what `GetInstanceMetricData`'s boundary walk admits.
///
/// WIDER than the instance rows of [`ALARM_METRICS`]: that list is the narrower
/// set of metrics an ALARM can watch, and `MetadataNoToken` is published by an
/// instance without being one of them.
pub const INSTANCE_METRIC_UNITS: &[(&str, &str)] = &[
    ("BurstCapacityPercentage", "Percent"),
    ("BurstCapacityTime", "Seconds"),
    ("CPUUtilization", "Percent"),
    ("NetworkIn", "Bytes"),
    ("NetworkOut", "Bytes"),
    ("StatusCheckFailed", "Count"),
    ("StatusCheckFailed_Instance", "Count"),
    ("StatusCheckFailed_System", "Count"),
    ("MetadataNoToken", "Count"),
    ("DiskReadOps", "Count"),
    ("DiskWriteOps", "Count"),
    ("DiskReadBytes", "Bytes"),
    ("DiskWriteBytes", "Bytes"),
    ("DiskTotalReadTime", "Seconds"),
    ("DiskTotalWriteTime", "Seconds"),
    ("DiskQueueLength", "Count"),
];

/// The unit the instance metric `metric_name` is published in, or `None` when
/// this catalog does not know the metric.
pub fn instance_metric_unit(metric_name: &str) -> Option<&'static str> {
    INSTANCE_METRIC_UNITS
        .iter()
        .find(|(name, _)| *name == metric_name)
        .map(|(_, unit)| *unit)
}

// ---------------------------------------------------------------------------
// Alarms
// ---------------------------------------------------------------------------

/// The `AlarmState` a newly created alarm reports, from the modelled `PutAlarm`
/// documentation: "When this action creates an alarm, the alarm state is
/// immediately set to INSUFFICIENT_DATA."
///
/// It is also where an alarm of this store STAYS until something moves it, and
/// the one call that moves one is `TestAlarm` — which puts an alarm into the
/// state its caller asks to test: no
/// metric an alarm can watch has a datapoint at all (this emulator runs no
/// instance, container, load balancer or database workload and receives no
/// measurement of one, which is why `GetInstanceMetricData`,
/// `GetContainerServiceMetricData` and `GetLoadBalancerMetricData` all answer an
/// empty `metricData`; `GetRelationalDatabaseMetricData` is still a scaffold with
/// its own issue), and
/// the modelled `Alarm$state`
/// documentation defines `INSUFFICIENT_DATA` as exactly that — "the metric is not
/// available, or not enough data is available for the metric to determine the
/// alarm state". Nothing here invents an evaluation that no metric data could
/// support.
///
/// A bucket's STORAGE metrics are the one series this store does measure — they
/// are a count and a size of what the bucket itself holds, not a measurement of a
/// running workload, so `GetBucketMetricData` computes them — and no alarm is
/// affected: [`ALARM_METRICS`] carries no bucket row, because the modelled
/// `PutAlarmRequest$metricName` documentation lists no bucket metric.
pub const ALARM_STATE_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The one `AlarmState` a notification trigger list must name, from the modelled
/// `PutAlarmRequest$notificationTriggers` documentation: "When you specify a
/// notification trigger, the ALARM state must be specified."
pub const ALARM_STATE_ALARM: &str = "ALARM";

/// The `treatMissingData` an alarm gets when the request names none, from that
/// member's own documentation: "If treatMissingData is not specified, the default
/// behavior of missing is used."
pub const TREAT_MISSING_DATA_DEFAULT: &str = "missing";

/// `Alarm$notificationEnabled` when the request names none, from the modelled
/// `PutAlarmRequest$notificationEnabled` documentation: "Notifications are
/// enabled by default if you don't specify this parameter."
pub const NOTIFICATION_ENABLED_DEFAULT: bool = true;

/// `Alarm$period`, the period in seconds the statistic is applied over.
///
/// From the modelled `PutAlarmRequest$evaluationPeriods` documentation: "Each
/// evaluation period is five minutes long." Five minutes is 300 seconds, which is
/// within the modelled `MetricPeriod` range (60..=86400).
pub const ALARM_PERIOD_SECONDS: i32 = 300;

/// The fewest `evaluationPeriods` an alarm accepts, from that member's own
/// documentation: "You can specify a minimum valuation period of 1 (5 minutes),
/// and a maximum evaluation period of 288 (24 hours)."
pub const MIN_EVALUATION_PERIODS: i32 = 1;
/// The most `evaluationPeriods` an alarm accepts. See [`MIN_EVALUATION_PERIODS`].
pub const MAX_EVALUATION_PERIODS: i32 = 288;

/// The most alarms one metric of one monitored resource may carry, from the
/// modelled `PutAlarmRequest$metricName` documentation: "You can configure up to
/// two alarms per metric."
pub const MAX_ALARMS_PER_METRIC: usize = 2;

/// How many alarms one `GetAlarms` page carries.
///
/// The modelled `GetAlarmsResult$nextPageToken` only says a paginated response
/// returns one, so the page size is this emulator's: small enough that a handful
/// of alarms exercises the walk.
pub const ALARM_PAGE_SIZE: usize = 10;

/// One metric an alarm can watch: which resource type publishes it, and the
/// `unit` and `statistic` the service reports on an alarm built from it.
///
/// `PutAlarm` takes neither the unit nor the statistic, yet `Alarm` reports both,
/// so each is derived from the metric — it is a property of the metric, not of
/// the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricSpec {
    /// A value of the modelled `MetricName` enum.
    pub name: &'static str,
    /// The resource type that publishes it, from the `RESOURCE_TYPE_*`
    /// constants.
    pub resource_type: &'static str,
    /// `Alarm$unit`: a value of the modelled `MetricUnit` enum.
    pub unit: &'static str,
    /// `Alarm$statistic`: a value of the modelled `MetricStatistic` enum.
    pub statistic: &'static str,
}

/// Every metric an alarm can watch, by resource type.
///
/// WHICH metrics each resource type publishes is the list in the modelled
/// `PutAlarmRequest$metricName` documentation ("The following metrics are
/// available for each resource type"), which is narrower than the `MetricName`
/// enum: `FreeableMemory` and `SwapUsage` are enum members that list names for no
/// resource type, so no alarm can be put on them.
///
/// The `unit` of each row is the "published `unit` value" that metric's own
/// documentation states, and the `statistic` is the one it names as most useful —
/// `Average` where it names several including `Average`, otherwise the first it
/// names. The sources are
/// `smithy_query(query="doc",shape="GetInstanceMetricDataRequest",member="metricName")`,
/// the same for `GetLoadBalancerMetricDataRequest` and for
/// `GetRelationalDatabaseMetricDataRequest`.
pub const ALARM_METRICS: &[MetricSpec] = &[
    // Instances.
    MetricSpec {
        name: "BurstCapacityPercentage",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Percent",
        statistic: "Average",
    },
    MetricSpec {
        name: "BurstCapacityTime",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Seconds",
        statistic: "Average",
    },
    MetricSpec {
        name: "CPUUtilization",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Percent",
        statistic: "Average",
    },
    MetricSpec {
        name: "NetworkIn",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "NetworkOut",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "StatusCheckFailed",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "StatusCheckFailed_Instance",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "StatusCheckFailed_System",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskReadOps",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskWriteOps",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskReadBytes",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskWriteBytes",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "DiskTotalReadTime",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Seconds",
        statistic: "Average",
    },
    MetricSpec {
        name: "DiskTotalWriteTime",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Seconds",
        statistic: "Average",
    },
    MetricSpec {
        name: "DiskQueueLength",
        resource_type: RESOURCE_TYPE_INSTANCE,
        unit: "Count",
        statistic: "Average",
    },
    // Load balancers.
    MetricSpec {
        name: "ClientTLSNegotiationErrorCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HealthyHostCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Average",
    },
    MetricSpec {
        name: "UnhealthyHostCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Average",
    },
    MetricSpec {
        name: "HTTPCode_LB_4XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_LB_5XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_Instance_2XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_Instance_3XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_Instance_4XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "HTTPCode_Instance_5XX_Count",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "InstanceResponseTime",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Seconds",
        statistic: "Average",
    },
    MetricSpec {
        name: "RejectedConnectionCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "RequestCount",
        resource_type: RESOURCE_TYPE_LOAD_BALANCER,
        unit: "Count",
        statistic: "Sum",
    },
    // Relational databases.
    MetricSpec {
        name: "CPUUtilization",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Percent",
        statistic: "Average",
    },
    MetricSpec {
        name: "DatabaseConnections",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Count",
        statistic: "Maximum",
    },
    MetricSpec {
        name: "DiskQueueDepth",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Count",
        statistic: "Sum",
    },
    MetricSpec {
        name: "FreeStorageSpace",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Bytes",
        statistic: "Sum",
    },
    MetricSpec {
        name: "NetworkReceiveThroughput",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Bytes/Second",
        statistic: "Average",
    },
    MetricSpec {
        name: "NetworkTransmitThroughput",
        resource_type: RESOURCE_TYPE_RELATIONAL_DATABASE,
        unit: "Bytes/Second",
        statistic: "Average",
    },
];

/// The metric `metric_name` names on a resource of `resource_type`, or `None`
/// when that resource type does not publish it — in which case no alarm can watch
/// it there.
pub fn alarm_metric(resource_type: &str, metric_name: &str) -> Option<&'static MetricSpec> {
    ALARM_METRICS
        .iter()
        .find(|spec| spec.resource_type == resource_type && spec.name == metric_name)
}

/// Every metric name a resource of `resource_type` publishes, comma separated —
/// what a refusal names so the caller can pick one that exists.
pub fn alarm_metric_names(resource_type: &str) -> String {
    ALARM_METRICS
        .iter()
        .filter(|spec| spec.resource_type == resource_type)
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A resource an alarm monitors, as the alarm has to report it.
///
/// From the modelled `PutAlarmRequest$monitoredResourceName` documentation:
/// "Instances, load balancers, and relational databases are the only Lightsail
/// resources that can currently be monitored by alarms." All three of those
/// resolve to one of these, each addressed the way its own operations address it
/// — see [`ComAmazonawsLightsailLightsail20161128State::monitored_resource`].
#[derive(Debug, Clone)]
pub struct MonitoredResource {
    pub name: String,
    pub arn: String,
    pub resource_type: &'static str,
    /// The monitored resource's own location: an alarm watches a resource of one
    /// region, so this is the region the alarm itself lives in.
    pub location: StoredLocation,
}

impl MonitoredResource {
    /// This resource as the modelled `MonitoredResourceInfo`.
    pub fn view(&self) -> MonitoredResourceInfo {
        MonitoredResourceInfo {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            resource_type: Some(self.resource_type.to_string()),
        }
    }
}

/// One stored alarm: the configuration `PutAlarm` accepted, the members the
/// service assigned, and the state the alarm is in.
#[derive(Debug, Clone)]
pub struct StoredAlarm {
    /// The alarm name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    /// The resource this alarm watches, as it stood when the alarm was put.
    pub monitored_resource: MonitoredResource,
    /// The metric this alarm watches, resolved against [`ALARM_METRICS`]: the
    /// `unit` and `statistic` every read reports come from it.
    pub metric: MetricSpec,
    pub comparison_operator: String,
    pub threshold: f64,
    pub evaluation_periods: i32,
    pub datapoints_to_alarm: Option<i32>,
    pub treat_missing_data: String,
    pub contact_protocols: Vec<String>,
    pub notification_triggers: Vec<String>,
    pub notification_enabled: bool,
    pub tags: Vec<Tag>,
    /// The current `AlarmState`. A create sets
    /// [`ALARM_STATE_INSUFFICIENT_DATA`]; an update leaves whatever is here
    /// alone, because "when you update an existing alarm, its state is left
    /// unchanged"; and a test sets the state it was asked to test.
    pub state: String,
}

impl StoredAlarm {
    /// This record as the modelled `Alarm`.
    pub fn view(&self) -> Alarm {
        Alarm {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_ALARM.to_string()),
            support_code: Some(self.support_code.clone()),
            monitored_resource_info: Some(self.monitored_resource.view()),
            comparison_operator: Some(self.comparison_operator.clone()),
            evaluation_periods: Some(self.evaluation_periods),
            period: Some(ALARM_PERIOD_SECONDS),
            threshold: Some(self.threshold),
            datapoints_to_alarm: self.datapoints_to_alarm,
            treat_missing_data: Some(self.treat_missing_data.clone()),
            statistic: Some(self.metric.statistic.to_string()),
            metric_name: Some(self.metric.name.to_string()),
            state: Some(self.state.clone()),
            unit: Some(self.metric.unit.to_string()),
            contact_protocols: Some(self.contact_protocols.clone()),
            notification_triggers: Some(self.notification_triggers.clone()),
            notification_enabled: Some(self.notification_enabled),
            tags: Some(self.tags.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Contact methods
// ---------------------------------------------------------------------------

/// The `ContactProtocol` values a contact method is created under, from the
/// modelled enum: an email address, or the mobile phone number SMS text messages
/// are sent to.
pub const CONTACT_PROTOCOL_EMAIL: &str = "Email";
/// See [`CONTACT_PROTOCOL_EMAIL`].
pub const CONTACT_PROTOCOL_SMS: &str = "SMS";

/// `ContactMethodStatus` values this slice uses, from the modelled enum's own
/// documentation: a contact method whose verification has not been completed yet
/// is `PendingVerification`, and one notifications can be sent through is
/// `Valid`.
///
/// The enum's `Invalid` member describes a contact method whose verification
/// EXPIRED ("an attempt was made to verify the contact method, but the
/// verification has expired"), which this store never produces: that expiry is
/// owed to a verification nobody completed, and nothing here completes or
/// abandons one.
pub const CONTACT_METHOD_STATUS_PENDING_VERIFICATION: &str = "PendingVerification";
/// See [`CONTACT_METHOD_STATUS_PENDING_VERIFICATION`].
pub const CONTACT_METHOD_STATUS_VALID: &str = "Valid";

/// The status a contact method of `protocol` is created in.
///
/// Only an EMAIL contact method has a verification to wait on: the modelled
/// `ContactMethodVerificationProtocol` enum — the one
/// `SendContactMethodVerification` takes — declares `Email` and nothing else, so
/// no other protocol has a verification to send, and a contact method of one is
/// usable as created.
pub fn contact_method_initial_status(protocol: &str) -> &'static str {
    if protocol == CONTACT_PROTOCOL_EMAIL {
        CONTACT_METHOD_STATUS_PENDING_VERIFICATION
    } else {
        CONTACT_METHOD_STATUS_VALID
    }
}

/// The Regions that serve the `SMS` contact protocol, from the modelled
/// `CreateContactMethodRequest$protocol` documentation: "The SMS protocol is
/// supported only in the following Amazon Web Services Regions", which then
/// lists exactly these six.
pub const SMS_SUPPORTED_REGIONS: &[&str] = &[
    "us-east-1",
    "us-west-2",
    "eu-west-1",
    "ap-northeast-1",
    "ap-southeast-1",
    "ap-southeast-2",
];

/// Whether `region` serves the `SMS` contact protocol; see
/// [`SMS_SUPPORTED_REGIONS`].
pub fn sms_supported_in(region: &str) -> bool {
    SMS_SUPPORTED_REGIONS.contains(&region)
}

/// The most digits a mobile phone number carries, from the modelled
/// `CreateContactMethodRequest$contactEndpoint` documentation: "Phone numbers
/// that follow this format can have a maximum of 15 digits, and they are
/// prefixed with the plus character (+) and the country code."
pub const MAX_E164_DIGITS: usize = 15;

/// Why `contact_endpoint` is not a destination of `protocol`, or `None` when it
/// is one.
///
/// The model constrains that member's LENGTH alone (1 to 256 characters), so the
/// form each protocol's destination takes is checked here: an `Email` endpoint is
/// an email address, and an `SMS` endpoint is the E.164 phone number that
/// member's own documentation requires.
pub fn contact_endpoint_rejection(protocol: &str, contact_endpoint: &str) -> Option<String> {
    if protocol == CONTACT_PROTOCOL_SMS {
        let digits = contact_endpoint.strip_prefix('+').unwrap_or_default();
        if digits.is_empty()
            || digits.len() > MAX_E164_DIGITS
            || !digits.chars().all(|c| c.is_ascii_digit())
        {
            return Some(format!(
                "contactEndpoint '{contact_endpoint}' is not a mobile phone number in E.164 \
                 format: the plus character and the country code, then at most \
                 {MAX_E164_DIGITS} digits in all"
            ));
        }
        return None;
    }
    if !is_email_address(contact_endpoint) {
        return Some(format!(
            "contactEndpoint '{contact_endpoint}' is not an email address: a local part, an @, \
             and a domain of at least two labels"
        ));
    }
    None
}

/// Whether `value` is an email address: one `@`, a non-empty local part, and a
/// domain of at least two non-empty labels, with no whitespace anywhere.
pub(super) fn is_email_address(value: &str) -> bool {
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = value.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if parts.next().is_some() || local.is_empty() || domain.is_empty() {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2 && labels.iter().all(|label| !label.is_empty())
}

/// The name a contact method is created under: the protocol it sends through and
/// a minted id.
///
/// The caller names no contact method, so the service assigns the name. It
/// satisfies the modelled `ContactMethod$name` pattern `^\w[\w\-]*\w$` and is
/// unique among the account's contact methods, because the id is.
pub fn contact_method_name(protocol: &str, id: &str) -> String {
    format!("{protocol}-{id}")
}

/// One stored contact method: the destination `CreateContactMethod` accepted,
/// the members the service assigned, and the verification status it is in.
#[derive(Debug, Clone)]
pub struct StoredContactMethod {
    /// The `ContactProtocol` this contact method sends through — with the Region
    /// it was created in, this record's store key.
    pub protocol: String,
    /// The destination the request named, kept verbatim.
    pub contact_endpoint: String,
    /// The name this store minted for the contact method; see
    /// [`contact_method_name`].
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    /// The current `ContactMethodStatus`; see
    /// [`contact_method_initial_status`].
    pub status: String,
    pub tags: Vec<Tag>,
}

impl StoredContactMethod {
    /// This record as the modelled `ContactMethod`.
    pub fn view(&self) -> ContactMethod {
        ContactMethod {
            contact_endpoint: Some(self.contact_endpoint.clone()),
            status: Some(self.status.clone()),
            protocol: Some(self.protocol.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_CONTACT_METHOD.to_string()),
            support_code: Some(self.support_code.clone()),
            tags: Some(self.tags.clone()),
        }
    }
}
