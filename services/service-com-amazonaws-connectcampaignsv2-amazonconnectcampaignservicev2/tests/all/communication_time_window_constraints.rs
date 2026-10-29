// SPDX-License-Identifier: CC0-1.0

//! The constraints that sit BEHIND a communication-time window's unions, end to end
//! over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handlers together. The rule under test, over every value a `TimeWindow` carries
//! behind the `OpenHours` and `RestrictedPeriods` unions, on both paths that set one
//! (`CreateCampaign` and `UpdateCampaignCommunicationTime`) and in every per-channel
//! window of `CommunicationTimeConfig`:
//!
//! * a value behind a union member is held to the `@pattern` and `@length` of the
//!   shape it targets — `Iso8601Time` (`^T\d{2}:\d{2}$`) for a `TimeRange`'s two
//!   times, `Iso8601Date` (`^\d{4}-\d{2}-\d{2}$`) for a `RestrictedPeriod`'s two dates
//!   and `RestrictedPeriodName` (len 1..127) for its name — so a value outside those
//!   bounds is HTTP 400 `ValidationException` naming the member, and the call stores
//!   nothing;
//! * every value inside them is accepted and read back by `DescribeCampaign`,
//!   including at each published bound of the `@length`.
//!
//! A union's own generated member schema carries no constraint traits, but the
//! structure behind it has its own, whose member schemas carry the model's
//! `@pattern`/`@length`, and the centralized constraint walk descends into it — so
//! these rules are enforced at the boundary, and the one leaf of a window the walk
//! leaves to the handlers is the `dailyHours` map KEY (its own suite,
//! `campaign_communication_time_update`). This suite is what pins that split: a
//! codegen or framework change that stopped descending past a union member would turn
//! every refusal row below into an accepted, PERSISTED value.
//!
//! The bounds and patterns the rows below are built from are read back out of
//! `smithy-model/` by [`the_window_rules_this_suite_asserts_are_the_models`], so no row
//! can be asserting a rule the model no longer declares.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, create_campaign_with, describe_campaign, error_code, router, Harness};

/// The region every case here signs for: one region holds every record a case reads,
/// and nothing in the rule under test is region-dependent.
const REGION: &str = "us-east-1";

/// `Iso8601Time`'s published `@pattern` — the shape `TimeRange$startTime` and
/// `$endTime` target — from `smithy_query(query="shape", shape="Iso8601Time")`, and
/// pinned to `smithy-model/` by [`the_window_rules_this_suite_asserts_are_the_models`].
const TIME_PATTERN: &str = r"^T\d{2}:\d{2}$";

/// `Iso8601Date`'s published `@pattern` — the shape `RestrictedPeriod$startDate` and
/// `$endDate` target — from `smithy_query(query="shape", shape="Iso8601Date")`.
const DATE_PATTERN: &str = r"^\d{4}-\d{2}-\d{2}$";

/// `RestrictedPeriodName`'s published `@length` bounds — the shape
/// `RestrictedPeriod$name` targets — from
/// `smithy_query(query="shape", shape="RestrictedPeriod")`.
const NAME_MIN_LEN: usize = 1;
const NAME_MAX_LEN: usize = 127;

/// Every per-channel window member of `CommunicationTimeConfig`, as
/// `smithy_query(query="shape", shape="CommunicationTimeConfig")` names them: the rule
/// holds behind each of them, so the rows below are spread across all four.
const CHANNEL_MEMBERS: [&str; 4] = ["telephony", "sms", "email", "whatsApp"];

/// A value a shape forbids: the case name, the value, and the requirement clause the
/// refusal must state for it.
type BadValue = (&'static str, String, String);

/// The requirement clause a refusal states for a value outside a `@pattern`.
fn pattern(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The requirement clause a refusal states for a value below a `@length` minimum.
fn min_length(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement clause a refusal states for a value past a `@length` maximum.
fn max_length(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// Values no `Iso8601Time` admits, each with the requirement it breaks.
///
/// The rows differ along what the shape's pattern generalises over: the empty value, a
/// value of another form entirely, the hour-and-minute form with no `T`, a lowercase
/// prefix, a single-digit hour, seconds appended, a trailing space, and the right form
/// with a separator the pattern does not spell.
fn times_outside_the_time_pattern() -> Vec<BadValue> {
    [
        ("the empty value", ""),
        ("words instead of a time", "not-a-time"),
        ("no T prefix", "08:00"),
        ("a lowercase prefix", "t08:00"),
        ("a single-digit hour", "T8:00"),
        ("seconds appended", "T08:00:00"),
        ("a trailing space", "T08:00 "),
        ("a dot for the separator", "T08.00"),
    ]
    .into_iter()
    .map(|(case, value)| (case, value.to_string(), pattern(TIME_PATTERN)))
    .collect()
}

/// Values no `Iso8601Date` admits, each with the requirement it breaks.
///
/// The rows differ along what the shape's pattern generalises over: the empty value, a
/// value of another form entirely, another calendar order, slashes for the separators,
/// unpadded month and day, no separators at all, a two-digit year, and a date with an
/// instant appended.
fn dates_outside_the_date_pattern() -> Vec<BadValue> {
    [
        ("the empty value", ""),
        ("words instead of a date", "nope"),
        ("day-first order", "01-01-2024"),
        ("slashes for the separators", "01/01/2024"),
        ("unpadded month and day", "2024-1-1"),
        ("no separators at all", "20240101"),
        ("a two-digit year", "24-01-01"),
        ("an instant appended", "2024-01-01T00:00:00Z"),
    ]
    .into_iter()
    .map(|(case, value)| (case, value.to_string(), pattern(DATE_PATTERN)))
    .collect()
}

/// A `communicationTimeConfig` whose `channel` window is `window`.
fn config_with(channel: &str, window: Value) -> Value {
    let mut config = json!({"localTimeZoneConfig": {"defaultTimeZone": "America/New_York"}});
    config[channel] = window;
    config
}

/// A window opening `day` with the single `ranges` list, carrying no restricted
/// periods.
fn open_hours(day: &str, ranges: Value) -> Value {
    json!({"openHours": {"dailyHours": {day: ranges}}})
}

/// A window opening one well-formed Monday range, restricted by `periods`.
fn restricted(periods: Value) -> Value {
    json!({
        "openHours": {"dailyHours": {"MONDAY": [{"startTime": "T08:00", "endTime": "T18:00"}]}},
        "restrictedPeriods": {"restrictedPeriodList": periods},
    })
}

/// The config every refusal case leaves untouched — well-formed in every window, so a
/// refusal that stored something is visible as a changed read.
fn kept_config() -> Value {
    let mut config = json!({"localTimeZoneConfig": {"defaultTimeZone": "UTC"}});
    for channel in CHANNEL_MEMBERS {
        config[channel] = restricted(json!([
            {"name": "holiday", "startDate": "2026-12-24", "endDate": "2026-12-26"}
        ]));
    }
    config
}

/// `UpdateCampaignCommunicationTime` for `id`, carrying `config`.
async fn update_time(router: &MultiServiceRouter, id: &str, config: &Value) -> (u16, Value) {
    call(
        router,
        REGION,
        "POST",
        &format!("/v2/campaigns/{id}/communication-time"),
        &json!({"communicationTimeConfig": config}),
    )
    .await
}

/// `CreateCampaign` carrying `config`, named `name` on `instance` — built here rather
/// than through the shared fixture because the cases below expect the create to be
/// REFUSED, and the fixture asserts it succeeded.
async fn create_with_config(
    router: &MultiServiceRouter,
    instance: &str,
    name: &str,
    config: &Value,
) -> (u16, Value) {
    let body = json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": {
            "telephony": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
            }
        },
        "communicationTimeConfig": config,
    });
    call(router, REGION, "PUT", "/v2/campaigns", &body).await
}

/// How many campaigns this region holds, as `ListCampaigns` reports them — how a case
/// asserts a refused create minted nothing.
async fn campaign_count(router: &MultiServiceRouter, case: &str) -> usize {
    let (status, body) = call(router, REGION, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected 200, got {status} {body}"
    );
    body.get("campaignSummaryList")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// Assert `response` is the modelled refusal of the member at `path`, stating
/// `requirement`.
fn assert_refused(case: &str, status: u16, response: &Value, path: &str, requirement: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {response}"
    );
    assert!(
        error_code(response).ends_with("ValidationException"),
        "case {case:?}: expected ValidationException, got {}",
        error_code(response)
    );
    let message = response["message"].as_str().unwrap_or("<no message>");
    assert!(
        message.contains(&format!("at '{path}'")),
        "case {case:?}: expected the refusal to name the member at {path:?}, got {message:?}"
    );
    assert!(
        message.contains(requirement),
        "case {case:?}: expected the refusal to state {requirement:?}, got {message:?}"
    );
}

/// The fixture a refusal case is driven against: the harness, the onboarded instance,
/// and a campaign holding a well-formed config in every window — so a refusal that
/// stored something is visible as a changed read.
struct Fixture {
    router: Harness,
    instance: &'static str,
    id: String,
    kept: Value,
}

impl Fixture {
    /// Onboard `instance` and create the campaign every case reads back.
    async fn new(instance: &'static str) -> Self {
        let router = router();
        let kept = kept_config();
        let id = create_campaign_with(
            &router,
            REGION,
            instance,
            "kept",
            json!({"communicationTimeConfig": kept.clone()}),
        )
        .await;
        Self {
            router,
            instance,
            id,
            kept,
        }
    }

    /// Drive one refusal case through both paths that set a communication time config,
    /// and assert neither stored anything.
    async fn assert_both_paths_refuse(
        &self,
        case: &str,
        config: &Value,
        path: &str,
        requirement: &str,
    ) {
        let before = campaign_count(&self.router, case).await;

        let update_case = format!("UpdateCampaignCommunicationTime: {case}");
        let (status, body) = update_time(&self.router, &self.id, config).await;
        assert_refused(&update_case, status, &body, path, requirement);
        let campaign = describe_campaign(&self.router, REGION, &self.id, &update_case).await;
        assert_eq!(
            campaign["communicationTimeConfig"], self.kept,
            "case {update_case:?}: expected the refusal to leave the stored config alone, \
             got {:?}",
            campaign["communicationTimeConfig"]
        );

        let create_case = format!("CreateCampaign: {case}");
        let (status, body) = create_with_config(
            &self.router,
            self.instance,
            &format!("born-{case}"),
            config,
        )
        .await;
        assert_refused(&create_case, status, &body, path, requirement);
        let count = campaign_count(&self.router, &create_case).await;
        assert_eq!(
            count, before,
            "case {create_case:?}: expected the refused create to mint nothing, got {count} \
             campaigns (was {before})"
        );
    }
}

/// The rules this suite's rows are built from are the model's.
///
/// Every value behind these two unions is judged off its structure's GENERATED schema,
/// which carries the model's own constraint traits; this pins the rows above to
/// `smithy-model/`, so a model refresh that moved a rule fails here instead of leaving
/// the suite asserting a stale one.
#[test]
fn the_window_rules_this_suite_asserts_are_the_models() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json"
    );
    let text = std::fs::read_to_string(path).expect("the service's Smithy model is readable");
    let model: Value = serde_json::from_str(&text).expect("the service's Smithy model is JSON");
    let shape =
        |name: &str| model["shapes"][format!("com.amazonaws.connectcampaignsv2#{name}")].clone();

    for (name, expected) in [
        ("Iso8601Time", TIME_PATTERN),
        ("Iso8601Date", DATE_PATTERN),
    ] {
        let traits = shape(name)["traits"].clone();
        assert_eq!(
            traits["smithy.api#pattern"],
            json!(expected),
            "case {name:?}: the modelled pattern must be the one this suite asserts, \
             got {traits}"
        );
    }

    let name_length = shape("RestrictedPeriodName")["traits"]["smithy.api#length"].clone();
    assert_eq!(
        name_length["min"],
        json!(NAME_MIN_LEN),
        "case \"RestrictedPeriodName @length min\": the modelled floor must be the one \
         this suite asserts, got {name_length}"
    );
    assert_eq!(
        name_length["max"],
        json!(NAME_MAX_LEN),
        "case \"RestrictedPeriodName @length max\": the modelled ceiling must be the one \
         this suite asserts, got {name_length}"
    );

    // The window members the rows are spread across are the ones the config declares.
    let members = shape("CommunicationTimeConfig")["members"].clone();
    for channel in CHANNEL_MEMBERS {
        assert!(
            members.get(channel).is_some(),
            "case {channel:?}: expected a per-channel window member on \
             CommunicationTimeConfig, got {members}"
        );
    }
}

/// A `TimeRange` time the shape forbids is refused by both paths that set a
/// communication time config, and neither stores it.
///
/// The rows are the two time-valued members of `TimeRange` crossed with
/// [`times_outside_the_time_pattern`], each sent in a different per-channel window and
/// at a different position of a multi-day, multi-range `dailyHours` map — so the rule
/// is asserted over the map KEY the bad row sits under and its position in the list as
/// well as over the value.
#[tokio::test]
async fn a_time_outside_the_modelled_pattern_is_refused_and_stores_nothing() {
    let fixture = Fixture::new("communication-time-window-times").await;

    // (the TimeRange member under test, the well-formed value its sibling carries)
    let members: [(&str, &str); 2] = [("startTime", "T18:00"), ("endTime", "T08:00")];

    for (offset, (member, sibling)) in members.into_iter().enumerate() {
        for (index, (value_case, value, requirement)) in
            times_outside_the_time_pattern().into_iter().enumerate()
        {
            // A different channel, day and list position per row: the rule holds
            // wherever behind the union the value sits, and the path the refusal names
            // follows it.
            let channel = CHANNEL_MEMBERS[(index + offset) % CHANNEL_MEMBERS.len()];
            let day = ["MONDAY", "FRIDAY"][index % 2];
            let position = index % 2;
            let bad = if member == "startTime" {
                json!({"startTime": value, "endTime": sibling})
            } else {
                json!({"startTime": sibling, "endTime": value})
            };
            let mut ranges = vec![json!({"startTime": "T06:00", "endTime": "T07:00"})];
            ranges.insert(position, bad);
            let config = config_with(channel, open_hours(day, Value::Array(ranges)));

            let case = format!("{member} sent {value_case} in {channel}");
            let path = format!(
                "communicationTimeConfig.{channel}.openHours.dailyHours.{day}.member.{}.member.{member}",
                position + 1
            );
            fixture
                .assert_both_paths_refuse(&case, &config, &path, &requirement)
                .await;
        }
    }
}

/// A row whose two times are both outside the pattern is refused ONCE, with a clause
/// for each — the accumulating envelope the framework's own walk renders, so a caller
/// learns everything wrong with the window it sent rather than one thing at a time.
#[tokio::test]
async fn every_bad_time_in_one_window_is_named_by_the_refusal() {
    let fixture = Fixture::new("communication-time-window-both").await;
    let config = config_with(
        "telephony",
        open_hours(
            "MONDAY",
            json!([{"startTime": "not-a-time", "endTime": "18:00"}]),
        ),
    );

    let case = "both times of one range outside the pattern";
    let (status, body) = update_time(&fixture.router, &fixture.id, &config).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    let message = body["message"].as_str().unwrap_or("<no message>");
    for member in ["startTime", "endTime"] {
        let path = format!(
            "communicationTimeConfig.telephony.openHours.dailyHours.MONDAY.member.1.member.\
             {member}"
        );
        assert!(
            message.contains(&format!("at '{path}'")),
            "case {case:?}: expected the refusal to name {path:?}, got {message:?}"
        );
    }
    assert!(
        message.starts_with("2 validation errors detected:"),
        "case {case:?}: expected both clauses in one envelope, got {message:?}"
    );
    let campaign = describe_campaign(&fixture.router, REGION, &fixture.id, case).await;
    assert_eq!(
        campaign["communicationTimeConfig"], fixture.kept,
        "case {case:?}: expected the refusal to leave the stored config alone, got {:?}",
        campaign["communicationTimeConfig"]
    );
}

/// A `RestrictedPeriod` date the shape forbids, and a name outside its `@length`, are
/// refused by both paths that set a communication time config, and neither stores
/// them.
///
/// The rows are the two date-valued members crossed with
/// [`dates_outside_the_date_pattern`], plus the name at one character below its
/// published minimum and one past its published maximum — each sent in a different
/// per-channel window and at a different position of the `restrictedPeriodList`.
#[tokio::test]
async fn a_restricted_period_outside_the_modelled_rules_is_refused_and_stores_nothing() {
    let fixture = Fixture::new("communication-time-window-periods").await;

    // The two date-valued members of `RestrictedPeriod`, crossed with every value the
    // shape they target forbids.
    let rows: Vec<(&str, String, String)> = ["startDate", "endDate"]
        .into_iter()
        .flat_map(|member| {
            dates_outside_the_date_pattern()
                .into_iter()
                .map(move |(case, value, _)| (member, format!("{member} sent {case}"), value))
        })
        .collect();

    for (index, (member, case, value)) in rows.into_iter().enumerate() {
        let channel = CHANNEL_MEMBERS[index % CHANNEL_MEMBERS.len()];
        let position = index % 2;
        let bad = if member == "startDate" {
            json!({"startDate": value, "endDate": "2026-12-26"})
        } else {
            json!({"startDate": "2026-12-24", "endDate": value})
        };
        let mut periods = vec![json!({"startDate": "2027-01-01", "endDate": "2027-01-02"})];
        periods.insert(position, bad);
        let config = config_with(channel, restricted(Value::Array(periods)));

        let case = format!("{case} in {channel}");
        let path = format!(
            "communicationTimeConfig.{channel}.restrictedPeriods.restrictedPeriodList.{}.member.{member}",
            position + 1
        );
        fixture
            .assert_both_paths_refuse(&case, &config, &path, &pattern(DATE_PATTERN))
            .await;
    }

    // The name's published bounds: one character below the minimum and one past the
    // maximum are both refused.
    let name_cases: [(&str, String, String); 2] = [
        (
            "a name one character below the length minimum",
            "a".repeat(NAME_MIN_LEN - 1),
            min_length(NAME_MIN_LEN),
        ),
        (
            "a name one character past the length maximum",
            "a".repeat(NAME_MAX_LEN + 1),
            max_length(NAME_MAX_LEN),
        ),
    ];
    for (case, name, requirement) in name_cases {
        let config = config_with(
            "telephony",
            restricted(json!([
                {"name": name, "startDate": "2026-12-24", "endDate": "2026-12-26"}
            ])),
        );
        let path = "communicationTimeConfig.telephony.restrictedPeriods.\
             restrictedPeriodList.1.member.name";
        fixture
            .assert_both_paths_refuse(case, &config, path, &requirement)
            .await;
    }
}

/// The other side of the rule: a window every shape admits is accepted and read back
/// whole, so the checks refuse what the shapes forbid and nothing else.
///
/// The rows span the forms a window takes — the first and last minute of a day, a day
/// opening several ranges, several days at once, no restricted periods, a period
/// spanning one day and one spanning a year boundary, a period with no name at all,
/// and a name at each published bound of its `@length` — in every per-channel window.
#[tokio::test]
async fn a_window_the_shapes_admit_is_accepted_and_read_back() {
    let router = router();
    let instance = "communication-time-window-accepted";
    let id = create_campaign_with(&router, REGION, instance, "accepting", json!({})).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "the first and last minute the pattern admits",
            config_with(
                "telephony",
                open_hours(
                    "SUNDAY",
                    json!([{"startTime": "T00:00", "endTime": "T23:59"}]),
                ),
            ),
        ),
        (
            "one day opening several ranges",
            config_with(
                "sms",
                open_hours(
                    "WEDNESDAY",
                    json!([
                        {"startTime": "T09:00", "endTime": "T12:00"},
                        {"startTime": "T13:00", "endTime": "T17:30"},
                        {"startTime": "T20:00", "endTime": "T21:00"}
                    ]),
                ),
            ),
        ),
        (
            "several days at once",
            config_with(
                "email",
                json!({"openHours": {"dailyHours": {
                    "MONDAY": [{"startTime": "T08:00", "endTime": "T18:00"}],
                    "TUESDAY": [{"startTime": "T08:15", "endTime": "T18:45"}],
                    "SATURDAY": [{"startTime": "T10:00", "endTime": "T14:00"}]
                }}}),
            ),
        ),
        (
            "a period spanning one day and a period spanning a year boundary",
            config_with(
                "whatsApp",
                restricted(json!([
                    {"name": "new year", "startDate": "2027-01-01", "endDate": "2027-01-01"},
                    {"startDate": "2026-12-28", "endDate": "2027-01-04"}
                ])),
            ),
        ),
        (
            "a name of exactly the length minimum",
            config_with(
                "telephony",
                restricted(json!([
                    {"name": "a".repeat(NAME_MIN_LEN), "startDate": "2026-07-04",
                     "endDate": "2026-07-04"}
                ])),
            ),
        ),
        (
            "a name of exactly the length maximum",
            config_with(
                "sms",
                restricted(json!([
                    {"name": "a".repeat(NAME_MAX_LEN), "startDate": "2026-07-04",
                     "endDate": "2026-07-05"}
                ])),
            ),
        ),
        (
            "every channel opened at once",
            {
                let mut config =
                    json!({"localTimeZoneConfig": {"defaultTimeZone": "Europe/Dublin"}});
                for channel in CHANNEL_MEMBERS {
                    config[channel] = restricted(json!([
                        {"name": "holiday", "startDate": "2026-12-24", "endDate": "2026-12-26"}
                    ]));
                }
                config
            },
        ),
    ];

    for (case, config) in &cases {
        let (status, body) = update_time(&router, &id, config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the config to be accepted, got {status} {body}"
        );
        let campaign = describe_campaign(&router, REGION, &id, case).await;
        assert_eq!(
            campaign["communicationTimeConfig"], *config,
            "case {case:?}: expected the stored config {config}, got {:?}",
            campaign["communicationTimeConfig"]
        );

        // The same config is accepted where a campaign is BORN with it, and read back
        // the same way.
        let (status, created) =
            create_with_config(&router, instance, &format!("born-{case}"), config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateCampaign to accept the config, got {status} \
             {created}"
        );
        let born = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateCampaign returned no id: {created}"));
        let campaign = describe_campaign(&router, REGION, born, case).await;
        assert_eq!(
            campaign["communicationTimeConfig"], *config,
            "case {case:?}: expected the born campaign to hold {config}, got {:?}",
            campaign["communicationTimeConfig"]
        );
    }
}
