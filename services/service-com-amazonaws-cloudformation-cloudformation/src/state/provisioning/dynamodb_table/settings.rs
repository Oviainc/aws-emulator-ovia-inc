// SPDX-License-Identifier: CC0-1.0

//! The properties of `AWS::DynamoDB::Table` that no member of `CreateTable` carries:
//! Amazon DynamoDB puts each of them into force through a CALL OF ITS OWN on the table that
//! already exists, and reports each through a READ of its own.
//!
//! ## The rule this module implements
//!
//! A stack creates its resources with the configuration their declarations ask for — every
//! property, not only the ones the owning service's create call happens to take. Where that
//! service applies a property through a separate call on the resource it has just created,
//! the provisioner makes that call, and the resource is reported created only once that
//! service's own read reports the setting IN FORCE. So a client that provisions a table by
//! stack and then reads one of these settings through DynamoDB — `DescribeTimeToLive`,
//! `DescribeContinuousBackups`, `DescribeContributorInsights`,
//! `DescribeKinesisStreamingDestination` — is answered what the template declared, and never
//! a table whose declared setting was dropped on the way.
//!
//! ## The settings, and the calls that put them into force
//!
//! Each entry of [`SETTINGS`] is one property of the type, with the DynamoDB call that
//! applies it and the read that reports it:
//!
//! * `TimeToLiveSpecification` — `UpdateTimeToLive`, reported by `DescribeTimeToLive`;
//! * `PointInTimeRecoverySpecification` — `UpdateContinuousBackups`, reported by
//!   `DescribeContinuousBackups`;
//! * `ContributorInsightsSpecification` — `UpdateContributorInsights`, reported by
//!   `DescribeContributorInsights`;
//! * `KinesisStreamSpecification` — `EnableKinesisStreamingDestination` /
//!   `UpdateKinesisStreamingDestination` / `DisableKinesisStreamingDestination`, reported by
//!   `DescribeKinesisStreamingDestination`.
//!
//! What is asked of each is computed from the pair (what is IN FORCE, what is DECLARED), not
//! from the declaration alone ([`calls`]): every one of these APIs changes a setting from one
//! state to another, and the ones that "enable or disable" a setting refuse a call that asks
//! for the state already in force. A declaration a deployment DROPPED asks for the setting
//! off, which those same calls express; a change none of them can express is reported as
//! having no in-place path ([`Change::NoPath`]), exactly as a change to a property
//! `UpdateTable` does not take is.
//!
//! `ImportSourceSpecification` is not here: it is not a property applied to a created table
//! at all but a different create — `ImportTable` rather than `CreateTable` — so a declaration
//! carrying it is still refused by the arm this module serves.

use serde_json::{Map, Value};

use super::{
    addressed, call_json_rpc, declared_in, Refusal, AWS_JSON_1_0, DYNAMODB_SIGNING_NAME,
    DYNAMODB_TARGET_PREFIX, TABLE_NAME_PROPERTY,
};

/// The operations that apply these settings, as an awsJson `X-Amz-Target` names them.
const UPDATE_TIME_TO_LIVE: &str = "UpdateTimeToLive";
const UPDATE_CONTINUOUS_BACKUPS: &str = "UpdateContinuousBackups";
const UPDATE_CONTRIBUTOR_INSIGHTS: &str = "UpdateContributorInsights";
const ENABLE_KINESIS_DESTINATION: &str = "EnableKinesisStreamingDestination";
const UPDATE_KINESIS_DESTINATION: &str = "UpdateKinesisStreamingDestination";
const DISABLE_KINESIS_DESTINATION: &str = "DisableKinesisStreamingDestination";

/// The property that carries the Time to Live setting, which is also the name of the member
/// `UpdateTimeToLive` takes it as.
const TIME_TO_LIVE_PROPERTY: &str = "TimeToLiveSpecification";

/// The property that carries the continuous-backups setting, which is also the name of the
/// member `UpdateContinuousBackups` takes it as.
const POINT_IN_TIME_RECOVERY_PROPERTY: &str = "PointInTimeRecoverySpecification";

/// The property that carries the CloudWatch Contributor Insights setting. The API takes no
/// such structure: its members are an ACTION and a mode ([`contributor_insights`]).
const CONTRIBUTOR_INSIGHTS_PROPERTY: &str = "ContributorInsightsSpecification";

/// The property that carries the Kinesis Data Streams destination the table replicates its
/// item-level changes to. The API addresses the destination by its stream ARN
/// ([`kinesis_destination`]).
const KINESIS_STREAM_PROPERTY: &str = "KinesisStreamSpecification";

/// The flag both the TTL specification and the Contributor Insights specification spell, and
/// the one the continuous-backups specification spells for the same thing.
const ENABLED_MEMBER: &str = "Enabled";
const POINT_IN_TIME_RECOVERY_ENABLED_MEMBER: &str = "PointInTimeRecoveryEnabled";

/// The attribute a TTL specification expires items by.
const ATTRIBUTE_NAME_MEMBER: &str = "AttributeName";

/// The number of days a continuous-backups specification keeps backups for, which can change
/// while the setting stays on.
const RECOVERY_PERIOD_MEMBER: &str = "RecoveryPeriodInDays";

/// `UpdateContributorInsights`'s own members, and the two values of its action
/// (`smithy_query(query="shape",shape="ContributorInsightsAction")`).
const CONTRIBUTOR_INSIGHTS_ACTION_MEMBER: &str = "ContributorInsightsAction";
const CONTRIBUTOR_INSIGHTS_MODE_MEMBER: &str = "ContributorInsightsMode";
const CONTRIBUTOR_INSIGHTS_ENABLE: &str = "ENABLE";
const CONTRIBUTOR_INSIGHTS_DISABLE: &str = "DISABLE";

/// The member the TEMPLATE's Contributor Insights specification spells the mode as, which the
/// API spells [`CONTRIBUTOR_INSIGHTS_MODE_MEMBER`].
const MODE_PROPERTY: &str = "Mode";

/// The members the Kinesis destination calls take: the stream the table replicates to, and
/// the configuration carrying the precision of the timestamp each replicated record reports.
const STREAM_ARN_MEMBER: &str = "StreamArn";
const ENABLE_KINESIS_CONFIGURATION_MEMBER: &str = "EnableKinesisStreamingConfiguration";
const UPDATE_KINESIS_CONFIGURATION_MEMBER: &str = "UpdateKinesisStreamingConfiguration";
const PRECISION_MEMBER: &str = "ApproximateCreationDateTimePrecision";

/// One setting of a table that is a DynamoDB call of its own: which property declares it,
/// what a declaration of it asks for, and how that service reports it.
pub(super) struct Setting {
    /// The property `AWS::DynamoDB::Table` publishes it as.
    pub(super) property: &'static str,
    /// What takes the setting from what is in force (`None` on a table being created) to what
    /// the declaration asks for (`None` where the declaration states none).
    change: fn(Option<&Value>, Option<&Value>) -> Change,
    /// The read that reports the setting.
    describe: &'static str,
    /// Every status that read reports for it — one per destination for the setting that is a
    /// list, one for each of the others.
    reported: fn(&Value) -> Vec<String>,
    /// The statuses that read reports while a change to the setting is still being applied,
    /// which is what holds a created resource short of ready ([`settling`]). A setting whose
    /// status enum declares no such value is reported in force the moment its call answers.
    settling: &'static [&'static str],
}

/// Every such setting of the type.
pub(super) const SETTINGS: &[Setting] = &[
    Setting {
        property: TIME_TO_LIVE_PROPERTY,
        change: time_to_live,
        describe: "DescribeTimeToLive",
        reported: time_to_live_status,
        // `TimeToLiveStatus`'s two transient values
        // (`smithy_query(query="shape",shape="TimeToLiveStatus")`): "It can take up to one
        // hour for the change to fully process" — the `UpdateTimeToLive` doc — so the
        // setting is reported in one of them until that service has applied the change.
        settling: &["ENABLING", "DISABLING"],
    },
    Setting {
        property: POINT_IN_TIME_RECOVERY_PROPERTY,
        change: point_in_time_recovery,
        describe: "DescribeContinuousBackups",
        reported: point_in_time_recovery_status,
        // `PointInTimeRecoveryStatus` declares `ENABLED` and `DISABLED` only
        // (`smithy_query(query="shape",shape="PointInTimeRecoveryStatus")`): there is no
        // status this setting is reported in while a change to it is being applied.
        settling: &[],
    },
    Setting {
        property: CONTRIBUTOR_INSIGHTS_PROPERTY,
        change: contributor_insights,
        describe: "DescribeContributorInsights",
        reported: contributor_insights_status,
        // `ContributorInsightsStatus`'s transient values
        // (`smithy_query(query="shape",shape="ContributorInsightsStatus")`).
        settling: &["ENABLING", "DISABLING"],
    },
    Setting {
        property: KINESIS_STREAM_PROPERTY,
        change: kinesis_destination,
        describe: "DescribeKinesisStreamingDestination",
        reported: kinesis_destination_status,
        // `DestinationStatus`'s transient values
        // (`smithy_query(query="shape",shape="DestinationStatus")`): a destination is
        // replicating in `ACTIVE`, and reports one of these while a change is in flight.
        settling: &["ENABLING", "DISABLING", "UPDATING"],
    },
];

/// One call that applies a setting: the DynamoDB operation, and its request body apart from
/// the member that addresses the table, which [`apply`] sets.
pub(super) struct SettingCall {
    operation: &'static str,
    request: Map<String, Value>,
}

/// What a declaration asks of ONE setting.
enum Change {
    /// The table already has what the declaration asks for, so nothing is called: these APIs
    /// change a setting from one state to another and refuse a call that asks for the state
    /// in force.
    Nothing,
    /// The calls that put the declared setting into force, in the order they must be made.
    Apply(Vec<SettingCall>),
    /// No call of this service can express the change on the table that exists.
    NoPath,
}

/// Whether `property` is one of these settings, which is what says the arm APPLIES it rather
/// than passing it to the create or refusing it.
pub(super) fn is_a_setting(property: &str) -> bool {
    SETTINGS.iter().any(|setting| setting.property == property)
}

/// The calls that take every one of these settings from what `in_force` has to what
/// `declared` asks for, or `None` where one of those changes has no call of this service to
/// express it.
///
/// `in_force` is `None` for a table being CREATED: nothing is in force on a table that does
/// not exist yet, so each declared setting is applied from off — which is the state
/// `CreateTable` leaves every one of them in, since that call carries none of them.
pub(super) fn calls(in_force: Option<&Value>, declared: &Value) -> Option<Vec<SettingCall>> {
    let mut calls = Vec::new();
    for setting in SETTINGS {
        let held = in_force.and_then(|properties| declared_in(properties, setting.property));
        let asked = declared_in(declared, setting.property);
        match (setting.change)(held, asked) {
            Change::Nothing => {}
            Change::Apply(mut made) => calls.append(&mut made),
            Change::NoPath => return None,
        }
    }
    Some(calls)
}

/// Make the calls `calls` names against the table `table_name`, in order.
///
/// `Err` is DynamoDB's own refusal of one of them, which is what the resource fails with: a
/// setting the declaration asked for that this service would not apply is not a resource the
/// stack may report created.
pub(super) async fn apply(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    table_name: &str,
    calls: &[SettingCall],
) -> Result<(), Refusal> {
    for call in calls {
        let mut request = call.request.clone();
        request.insert(
            TABLE_NAME_PROPERTY.to_string(),
            Value::String(table_name.to_string()),
        );
        call_json_rpc(
            ctx,
            DYNAMODB_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{DYNAMODB_TARGET_PREFIX}.{}", call.operation),
            &Value::Object(request),
        )
        .await?;
    }
    Ok(())
}

/// The status DynamoDB reports for a setting of `table_name` that `declared` asked for and
/// that service is still applying, or `None` where every one of them is in force.
///
/// This is what holds a created resource short of ready: the table EXISTS from the moment
/// `CreateTable` answers, but a setting its declaration asked for is not in force while that
/// service still reports it coming into force, and a resource reported created is one whose
/// whole declared configuration a client can read back.
///
/// Only the settings the DECLARATION carries are asked about: a setting nothing declared is
/// no part of what this resource's creation owes, so whatever that service reports about it
/// holds nothing back. `declared` is `None` where the records do not say what the resource
/// was declared with, and every setting is then asked about rather than none, since nothing
/// says which of them the creation owes.
///
/// A read that service REFUSES says nothing about the setting — one unanswered read is no
/// evidence about the table — so it leaves the question unanswered and the create's own
/// answer stands.
pub(super) async fn settling(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    table_name: &str,
    declared: Option<&Value>,
) -> Option<String> {
    for setting in SETTINGS {
        if setting.settling.is_empty() {
            continue;
        }
        if declared.is_some_and(|properties| declared_in(properties, setting.property).is_none()) {
            continue;
        }
        let Ok(answered) = call_json_rpc(
            ctx,
            DYNAMODB_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{DYNAMODB_TARGET_PREFIX}.{}", setting.describe),
            &addressed(table_name),
        )
        .await
        else {
            continue;
        };
        if let Some(status) = (setting.reported)(&answered)
            .into_iter()
            .find(|status| setting.settling.contains(&status.as_str()))
        {
            return Some(status);
        }
    }
    None
}

/// `TimeToLiveSpecification`, applied by `UpdateTimeToLive`: that API "enables or disables
/// Time to Live (TTL) for the specified table", and the template's members ARE its own
/// (`Enabled`, `AttributeName`), so a declaration travels into the call as written.
///
/// The call is made only where the table is not already in the state the declaration asks
/// for — it is refused for a call asking for the state in force — and a declaration a
/// deployment DROPPED asks for TTL off, which the same call expresses over the attribute the
/// setting is in force for. What it cannot express is keeping TTL on over ANOTHER attribute:
/// the attribute of a table whose TTL is on is not something this API moves.
fn time_to_live(in_force: Option<&Value>, declared: Option<&Value>) -> Change {
    let held = flag(in_force, ENABLED_MEMBER);
    match declared {
        Some(specification) => {
            let wanted = flag(declared, ENABLED_MEMBER);
            if wanted == held {
                let attribute = text(declared, ATTRIBUTE_NAME_MEMBER);
                if wanted && attribute != text(in_force, ATTRIBUTE_NAME_MEMBER) {
                    return Change::NoPath;
                }
                return Change::Nothing;
            }
            Change::Apply(vec![one_member(
                UPDATE_TIME_TO_LIVE,
                TIME_TO_LIVE_PROPERTY,
                specification.clone(),
            )])
        }
        None => {
            if !held {
                return Change::Nothing;
            }
            let Some(attribute) = text(in_force, ATTRIBUTE_NAME_MEMBER) else {
                return Change::NoPath;
            };
            Change::Apply(vec![one_member(
                UPDATE_TIME_TO_LIVE,
                TIME_TO_LIVE_PROPERTY,
                serde_json::json!({ ENABLED_MEMBER: false, ATTRIBUTE_NAME_MEMBER: attribute }),
            )])
        }
    }
}

/// `PointInTimeRecoverySpecification`, applied by `UpdateContinuousBackups`, whose own
/// member is spelled identically, so a declaration travels into the call as written.
///
/// The setting is the pair (whether recovery is on, how many days it keeps), and the period
/// can change while it stays on, so a declaration differing in either is applied; a
/// declaration a deployment dropped asks for it off.
fn point_in_time_recovery(in_force: Option<&Value>, declared: Option<&Value>) -> Change {
    let held = flag(in_force, POINT_IN_TIME_RECOVERY_ENABLED_MEMBER);
    match declared {
        Some(specification) => {
            let wanted = flag(declared, POINT_IN_TIME_RECOVERY_ENABLED_MEMBER);
            let same_period = member(declared, RECOVERY_PERIOD_MEMBER)
                == member(in_force, RECOVERY_PERIOD_MEMBER);
            if wanted == held && (!wanted || same_period) {
                return Change::Nothing;
            }
            Change::Apply(vec![one_member(
                UPDATE_CONTINUOUS_BACKUPS,
                POINT_IN_TIME_RECOVERY_PROPERTY,
                specification.clone(),
            )])
        }
        None => {
            if !held {
                return Change::Nothing;
            }
            Change::Apply(vec![one_member(
                UPDATE_CONTINUOUS_BACKUPS,
                POINT_IN_TIME_RECOVERY_PROPERTY,
                serde_json::json!({ POINT_IN_TIME_RECOVERY_ENABLED_MEMBER: false }),
            )])
        }
    }
}

/// `ContributorInsightsSpecification`, applied by `UpdateContributorInsights`, which takes no
/// such structure: its members are an ACTION (`ENABLE` / `DISABLE`) and the mode the
/// declaration asks for, so the declared flag is mapped to the action.
fn contributor_insights(in_force: Option<&Value>, declared: Option<&Value>) -> Change {
    let held = flag(in_force, ENABLED_MEMBER);
    let wanted = flag(declared, ENABLED_MEMBER);
    if declared.is_none() {
        if !held {
            return Change::Nothing;
        }
        return Change::Apply(vec![insights_action(false, None)]);
    }
    let wanted_mode = text(declared, MODE_PROPERTY);
    if wanted == held && (!wanted || wanted_mode == text(in_force, MODE_PROPERTY)) {
        return Change::Nothing;
    }
    let mode = wanted.then_some(wanted_mode).flatten();
    Change::Apply(vec![insights_action(wanted, mode)])
}

/// One `UpdateContributorInsights` call: the action the declared flag asks for, with the mode
/// beside it where the declaration named one and the setting is being turned on.
fn insights_action(enable: bool, mode: Option<String>) -> SettingCall {
    let mut request = Map::new();
    request.insert(
        CONTRIBUTOR_INSIGHTS_ACTION_MEMBER.to_string(),
        Value::String(
            if enable {
                CONTRIBUTOR_INSIGHTS_ENABLE
            } else {
                CONTRIBUTOR_INSIGHTS_DISABLE
            }
            .to_string(),
        ),
    );
    if let Some(mode) = mode {
        request.insert(
            CONTRIBUTOR_INSIGHTS_MODE_MEMBER.to_string(),
            Value::String(mode),
        );
    }
    SettingCall {
        operation: UPDATE_CONTRIBUTOR_INSIGHTS,
        request,
    }
}

/// `KinesisStreamSpecification`, the destination the table replicates its item-level changes
/// to, which three calls of this service address by the stream's ARN: the destination is
/// registered (`EnableKinesisStreamingDestination`), its precision changed
/// (`UpdateKinesisStreamingDestination`) or it is withdrawn
/// (`DisableKinesisStreamingDestination`).
///
/// A declaration naming ANOTHER stream than the one in force is the pair of calls a client
/// makes for it: the destination in force is withdrawn and the declared one registered.
fn kinesis_destination(in_force: Option<&Value>, declared: Option<&Value>) -> Change {
    let held_arn = text(in_force, STREAM_ARN_MEMBER);
    let wanted_arn = text(declared, STREAM_ARN_MEMBER);
    match (held_arn.as_deref(), wanted_arn.as_deref()) {
        // The declaration names the destination the table already replicates to, so only the
        // precision can still differ.
        (Some(held), Some(wanted)) if held == wanted => {
            if text(declared, PRECISION_MEMBER) == text(in_force, PRECISION_MEMBER) {
                return Change::Nothing;
            }
            Change::Apply(vec![destination_call(
                UPDATE_KINESIS_DESTINATION,
                UPDATE_KINESIS_CONFIGURATION_MEMBER,
                wanted,
                text(declared, PRECISION_MEMBER),
            )])
        }
        (Some(held), Some(wanted)) => Change::Apply(vec![
            destination_call(
                DISABLE_KINESIS_DESTINATION,
                ENABLE_KINESIS_CONFIGURATION_MEMBER,
                held,
                None,
            ),
            destination_call(
                ENABLE_KINESIS_DESTINATION,
                ENABLE_KINESIS_CONFIGURATION_MEMBER,
                wanted,
                text(declared, PRECISION_MEMBER),
            ),
        ]),
        (Some(held), None) => Change::Apply(vec![destination_call(
            DISABLE_KINESIS_DESTINATION,
            ENABLE_KINESIS_CONFIGURATION_MEMBER,
            held,
            None,
        )]),
        // A declaration that states the property but names no stream names no destination
        // this service could register: the call is made as declared, and that service's own
        // refusal of it is what the resource fails with, rather than the property being
        // dropped.
        (None, _) if declared.is_some() => Change::Apply(vec![destination_call(
            ENABLE_KINESIS_DESTINATION,
            ENABLE_KINESIS_CONFIGURATION_MEMBER,
            wanted_arn.as_deref().unwrap_or_default(),
            text(declared, PRECISION_MEMBER),
        )]),
        (None, _) => Change::Nothing,
    }
}

/// One call addressing a Kinesis destination: the stream's ARN, and the configuration member
/// this call carries the precision in where one is being asked for.
fn destination_call(
    operation: &'static str,
    configuration_member: &'static str,
    stream_arn: &str,
    precision: Option<String>,
) -> SettingCall {
    let mut request = Map::new();
    request.insert(
        STREAM_ARN_MEMBER.to_string(),
        Value::String(stream_arn.to_string()),
    );
    if let Some(precision) = precision {
        request.insert(
            configuration_member.to_string(),
            serde_json::json!({ PRECISION_MEMBER: precision }),
        );
    }
    SettingCall { operation, request }
}

/// One call whose body is a single member carrying the declared specification.
fn one_member(operation: &'static str, member: &str, specification: Value) -> SettingCall {
    let mut request = Map::new();
    request.insert(member.to_string(), specification);
    SettingCall { operation, request }
}

/// Whether a specification's flag `name` is set. A specification that is absent, or that
/// states no such member, states no such setting — which is the state `CreateTable` leaves
/// every one of these in.
fn flag(specification: Option<&Value>, name: &str) -> bool {
    member(specification, name)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// One member of a specification, or `None` where the specification states none.
fn member<'v>(specification: Option<&'v Value>, name: &str) -> Option<&'v Value> {
    specification
        .and_then(|specification| specification.get(name))
        .filter(|value| !value.is_null())
}

/// One member of a specification as text.
fn text(specification: Option<&Value>, name: &str) -> Option<String> {
    member(specification, name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The status `DescribeTimeToLive` reports (`TimeToLiveDescription$TimeToLiveStatus`).
fn time_to_live_status(answered: &Value) -> Vec<String> {
    reported_at(answered, &["TimeToLiveDescription", "TimeToLiveStatus"])
}

/// The status `DescribeContinuousBackups` reports
/// (`ContinuousBackupsDescription$PointInTimeRecoveryDescription$PointInTimeRecoveryStatus`).
fn point_in_time_recovery_status(answered: &Value) -> Vec<String> {
    reported_at(
        answered,
        &[
            "ContinuousBackupsDescription",
            "PointInTimeRecoveryDescription",
            "PointInTimeRecoveryStatus",
        ],
    )
}

/// The status `DescribeContributorInsights` reports, which is a member of the answer itself.
fn contributor_insights_status(answered: &Value) -> Vec<String> {
    reported_at(answered, &["ContributorInsightsStatus"])
}

/// The status `DescribeKinesisStreamingDestination` reports for EACH destination the table
/// holds a record for: the setting is a list, and a change to any of its records is a change
/// to the table's declared configuration.
fn kinesis_destination_status(answered: &Value) -> Vec<String> {
    answered
        .get("KinesisDataStreamDestinations")
        .and_then(Value::as_array)
        .map(|destinations| {
            destinations
                .iter()
                .filter_map(|destination| {
                    destination
                        .get("DestinationStatus")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The text at `path` in an answer, as the one-element list [`Setting::reported`] answers
/// for a setting a table carries one status for.
fn reported_at(answered: &Value, path: &[&str]) -> Vec<String> {
    let mut at = answered;
    for member in path {
        match at.get(member) {
            Some(next) => at = next,
            None => return Vec::new(),
        }
    }
    at.as_str().map(str::to_string).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The operations one set of calls makes, in order, each with its request body.
    fn made(calls: &[SettingCall]) -> Vec<(&'static str, Value)> {
        calls
            .iter()
            .map(|call| (call.operation, Value::Object(call.request.clone())))
            .collect()
    }

    /// One row of [`a_declared_setting_is_applied_by_its_own_call`]: the case, what is in
    /// force (`None` for a table being created), what is declared, and the calls that are
    /// due — `None` where the change has no in-place path.
    type Row = (
        &'static str,
        Option<Value>,
        Value,
        Option<Vec<(&'static str, Value)>>,
    );

    /// Every setting of the type: what a declaration of it asks of DynamoDB, over a table
    /// being created and over one that already has a setting in force.
    #[test]
    fn a_declared_setting_is_applied_by_its_own_call() {
        let rows: Vec<Row> = vec![
            (
                "a created table whose declaration enables TTL",
                None,
                serde_json::json!({
                    "TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": true},
                }),
                Some(vec![(
                    UPDATE_TIME_TO_LIVE,
                    serde_json::json!({
                        "TimeToLiveSpecification": {"AttributeName": "expiresAt",
                                                    "Enabled": true},
                    }),
                )]),
            ),
            (
                "a created table whose declaration leaves TTL off: nothing to apply",
                None,
                serde_json::json!({
                    "TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": false},
                }),
                Some(Vec::new()),
            ),
            (
                "TTL dropped from the declaration: turned off over the attribute in force",
                Some(serde_json::json!({
                    "TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": true},
                })),
                serde_json::json!({}),
                Some(vec![(
                    UPDATE_TIME_TO_LIVE,
                    serde_json::json!({
                        "TimeToLiveSpecification": {"AttributeName": "expiresAt",
                                                    "Enabled": false},
                    }),
                )]),
            ),
            (
                "TTL kept on over another attribute: no call moves it",
                Some(serde_json::json!({
                    "TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": true},
                })),
                serde_json::json!({
                    "TimeToLiveSpecification": {"AttributeName": "ttl", "Enabled": true},
                }),
                None,
            ),
            (
                "TTL already in force exactly as declared: nothing to apply",
                Some(serde_json::json!({
                    "TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": true},
                })),
                serde_json::json!({
                    "TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": true},
                }),
                Some(Vec::new()),
            ),
            (
                "a created table whose declaration asks for continuous backups",
                None,
                serde_json::json!({
                    "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true,
                                                         "RecoveryPeriodInDays": 7},
                }),
                Some(vec![(
                    UPDATE_CONTINUOUS_BACKUPS,
                    serde_json::json!({
                        "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true,
                                                             "RecoveryPeriodInDays": 7},
                    }),
                )]),
            ),
            (
                "the recovery period changed while recovery stays on",
                Some(serde_json::json!({
                    "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true,
                                                         "RecoveryPeriodInDays": 7},
                })),
                serde_json::json!({
                    "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true,
                                                         "RecoveryPeriodInDays": 35},
                }),
                Some(vec![(
                    UPDATE_CONTINUOUS_BACKUPS,
                    serde_json::json!({
                        "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true,
                                                             "RecoveryPeriodInDays": 35},
                    }),
                )]),
            ),
            (
                "a created table asking for Contributor Insights in a declared mode",
                None,
                serde_json::json!({
                    "ContributorInsightsSpecification": {"Enabled": true,
                                                         "Mode": "ACCESSED_AND_THROTTLED_KEYS"},
                }),
                Some(vec![(
                    UPDATE_CONTRIBUTOR_INSIGHTS,
                    serde_json::json!({
                        "ContributorInsightsAction": "ENABLE",
                        "ContributorInsightsMode": "ACCESSED_AND_THROTTLED_KEYS",
                    }),
                )]),
            ),
            (
                "Contributor Insights dropped from the declaration: disabled",
                Some(serde_json::json!({
                    "ContributorInsightsSpecification": {"Enabled": true},
                })),
                serde_json::json!({}),
                Some(vec![(
                    UPDATE_CONTRIBUTOR_INSIGHTS,
                    serde_json::json!({"ContributorInsightsAction": "DISABLE"}),
                )]),
            ),
            (
                "a created table replicating to a declared Kinesis stream",
                None,
                serde_json::json!({
                    "KinesisStreamSpecification": {
                        "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/changes",
                        "ApproximateCreationDateTimePrecision": "MICROSECOND",
                    },
                }),
                Some(vec![(
                    ENABLE_KINESIS_DESTINATION,
                    serde_json::json!({
                        "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/changes",
                        "EnableKinesisStreamingConfiguration": {
                            "ApproximateCreationDateTimePrecision": "MICROSECOND",
                        },
                    }),
                )]),
            ),
            (
                "the declared destination moved to another stream: withdrawn, then registered",
                Some(serde_json::json!({
                    "KinesisStreamSpecification": {
                        "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/old",
                    },
                })),
                serde_json::json!({
                    "KinesisStreamSpecification": {
                        "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/new",
                    },
                }),
                Some(vec![
                    (
                        DISABLE_KINESIS_DESTINATION,
                        serde_json::json!({
                            "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/old",
                        }),
                    ),
                    (
                        ENABLE_KINESIS_DESTINATION,
                        serde_json::json!({
                            "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/new",
                        }),
                    ),
                ]),
            ),
            (
                "only the replication precision changed",
                Some(serde_json::json!({
                    "KinesisStreamSpecification": {
                        "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/changes",
                        "ApproximateCreationDateTimePrecision": "MILLISECOND",
                    },
                })),
                serde_json::json!({
                    "KinesisStreamSpecification": {
                        "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/changes",
                        "ApproximateCreationDateTimePrecision": "MICROSECOND",
                    },
                }),
                Some(vec![(
                    UPDATE_KINESIS_DESTINATION,
                    serde_json::json!({
                        "StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/changes",
                        "UpdateKinesisStreamingConfiguration": {
                            "ApproximateCreationDateTimePrecision": "MICROSECOND",
                        },
                    }),
                )]),
            ),
            (
                "a table declaring none of them: nothing is called",
                None,
                serde_json::json!({"KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}]}),
                Some(Vec::new()),
            ),
        ];
        for (case, in_force, declared, expected) in &rows {
            let answered = calls(in_force.as_ref(), declared);
            let reported = answered.as_deref().map(made);
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected the calls {expected:?}, got {reported:?}"
            );
        }
    }

    /// Every setting's property is one this arm applies, so the refusal of an unapplied
    /// property cannot name one of them.
    #[test]
    fn every_setting_is_an_applied_property() {
        for setting in SETTINGS {
            assert!(
                is_a_setting(setting.property),
                "case {:?}: expected the setting's property to be an applied one, got none",
                setting.property
            );
        }
    }

    /// The status each read reports is found where that operation's output carries it.
    #[test]
    fn the_reported_status_is_read_off_the_services_own_answer() {
        let rows: &[(&str, &str, Value, &[&str])] = &[
            (
                "time to live",
                TIME_TO_LIVE_PROPERTY,
                serde_json::json!({"TimeToLiveDescription": {"TimeToLiveStatus": "ENABLING",
                                                             "AttributeName": "expiresAt"}}),
                &["ENABLING"],
            ),
            (
                "continuous backups",
                POINT_IN_TIME_RECOVERY_PROPERTY,
                serde_json::json!({"ContinuousBackupsDescription": {
                    "ContinuousBackupsStatus": "ENABLED",
                    "PointInTimeRecoveryDescription": {"PointInTimeRecoveryStatus": "ENABLED"},
                }}),
                &["ENABLED"],
            ),
            (
                "contributor insights",
                CONTRIBUTOR_INSIGHTS_PROPERTY,
                serde_json::json!({"ContributorInsightsStatus": "ENABLED"}),
                &["ENABLED"],
            ),
            (
                "every Kinesis destination record",
                KINESIS_STREAM_PROPERTY,
                serde_json::json!({"KinesisDataStreamDestinations": [
                    {"StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/old",
                     "DestinationStatus": "DISABLED"},
                    {"StreamArn": "arn:aws:kinesis:us-east-1:000000000000:stream/new",
                     "DestinationStatus": "ENABLING"},
                ]}),
                &["DISABLED", "ENABLING"],
            ),
            (
                "a setting the answer reports nothing about",
                TIME_TO_LIVE_PROPERTY,
                serde_json::json!({}),
                &[],
            ),
        ];
        for (case, property, answered, expected) in rows {
            let setting = SETTINGS
                .iter()
                .find(|setting| setting.property == *property)
                .expect("the property is one of the settings");
            let reported = (setting.reported)(answered);
            let wanted: Vec<String> = expected.iter().map(|status| (*status).to_string()).collect();
            assert_eq!(
                reported, wanted,
                "case {case:?}: expected the statuses {wanted:?}, got {reported:?}"
            );
        }
    }
}
