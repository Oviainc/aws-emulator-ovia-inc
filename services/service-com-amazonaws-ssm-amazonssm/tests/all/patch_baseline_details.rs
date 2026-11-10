// SPDX-License-Identifier: CC0-1.0

//! What a read of one patch baseline reports, end to end: `CreatePatchBaseline` mints a
//! baseline and `GetPatchBaseline` reports it.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call_in_region`], [`error_code`],
//! [`error_message`]) dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>`
//! through the multi-service router. Nothing reaches into the state struct, so a response
//! that is not computed from persisted state cannot pass: every read follows a write.
//!
//! The rule under test is that a read of a baseline reports, for the `BaselineId` the
//! request names, exactly the members the write of that baseline persisted — with the
//! documented default of every member the write left out — and refuses an id that names no
//! baseline reachable through the endpoint. The suites are tables whose rows differ along
//! what the rule generalises over: which optional members a write supplies, how many
//! entries each list carries, the operating system, the Region the endpoint is, which
//! spelling names the baseline (its own id or its full ARN), whether the baseline is the
//! account's own or one Systems Manager predefined, and the way an id misses.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, error_message, router, ACCOUNT, REGION};

/// The `OperatingSystem` values the model's enum declares
/// (`smithy_query(query="shape", shape="OperatingSystem")`).
const OPERATING_SYSTEMS: &[&str] = &[
    "ALMA_LINUX",
    "AMAZON_LINUX",
    "AMAZON_LINUX_2",
    "AMAZON_LINUX_2022",
    "AMAZON_LINUX_2023",
    "CENTOS",
    "DEBIAN",
    "MACOS",
    "ORACLE_LINUX",
    "RASPBIAN",
    "REDHAT_ENTERPRISE_LINUX",
    "ROCKY_LINUX",
    "SUSE",
    "UBUNTU",
    "WINDOWS",
];

/// The operating system a baseline whose create named none applies to: "Defines the
/// operating system the patch baseline applies to. The default value is WINDOWS"
/// (`smithy_query(query="doc", shape="CreatePatchBaselineRequest",
/// member="OperatingSystem")`).
const DEFAULT_OPERATING_SYSTEM: &str = "WINDOWS";

/// The compliance level of a baseline whose create named none: "The default value is
/// UNSPECIFIED" (`CreatePatchBaselineRequest$ApprovedPatchesComplianceLevel`).
const DEFAULT_COMPLIANCE_LEVEL: &str = "UNSPECIFIED";

/// What Patch Manager does with a rejected patch when the create named no action: "This is
/// the default action if no option is specified"
/// (`CreatePatchBaselineRequest$RejectedPatchesAction`).
const DEFAULT_REJECTED_PATCHES_ACTION: &str = "ALLOW_AS_DEPENDENCY";

/// The published ceiling of `ApprovedPatches` and `RejectedPatches`
/// (`smithy_query(query="shape", shape="PatchIdList")`: `length 0..50`).
const MAX_PATCH_IDS: usize = 50;

/// The published ceiling of a rule group's own list (`smithy_query(query="shape",
/// shape="PatchRuleGroup")`: `PatchRules`, `length 0..10`).
const MAX_PATCH_RULES: usize = 10;

/// The published ceiling of `Sources` (`smithy_query(query="shape",
/// shape="PatchSourceList")`: `length 0..20`).
const MAX_SOURCES: usize = 20;

/// The sentence `DoesNotExistException` carries for a `BaselineId` that names no patch
/// baseline reachable through the endpoint: fixed text for this error class, naming the
/// resource class and interpolating nothing from the request. Spelled out here rather than
/// read from the crate, so the wording itself is what the assertions check.
const NOT_FOUND_MESSAGE: &str = "Patch Baseline to be retrieved does not exist.";

/// `CreatePatchBaseline` at `region`'s endpoint with `body`.
async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "CreatePatchBaseline", body).await
}

/// The same write, asserting it succeeded, and returning the `BaselineId` it minted.
async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    body: &Value,
) -> String {
    let (status, response) = create(router, region, body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreatePatchBaseline {body} must succeed, got {status} {response}"
    );
    response["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a create reports a BaselineId, got {response}"))
        .to_string()
}

/// `GetPatchBaseline` at `region`'s endpoint for `baseline_id`.
async fn get(router: &MultiServiceRouter, region: &str, baseline_id: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GetPatchBaseline",
        &json!({ "BaselineId": baseline_id }),
    )
    .await
}

/// The same read, asserting it succeeded.
async fn get_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    baseline_id: &str,
) -> Value {
    let (status, response) = get(router, region, baseline_id).await;
    assert_eq!(
        status, 200,
        "case {case}: GetPatchBaseline {baseline_id:?} must succeed, got {status} {response}"
    );
    response
}

/// `GetDefaultPatchBaseline` at `region`'s endpoint for `operating_system`, returning the
/// `BaselineId` it reports — the full ARN of the baseline Systems Manager predefines while
/// nothing has registered another.
async fn predefined_baseline_id(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operating_system: &str,
) -> String {
    let (status, response) = call_in_region(
        router,
        region,
        "GetDefaultPatchBaseline",
        &json!({ "OperatingSystem": operating_system }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetDefaultPatchBaseline must succeed, got {status} {response}"
    );
    response["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a read reports a BaselineId, got {response}"))
        .to_string()
}

/// The ARN of the patch baseline `baseline_id` names in `region`, the form the account's
/// own ARNs carry.
fn baseline_arn(region: &str, baseline_id: &str) -> String {
    format!("arn:aws:ssm:{region}:{ACCOUNT}:patchbaseline/{baseline_id}")
}

/// One approval rule selecting patches of `classification`, approving them after a day —
/// the shape `PatchRule` requires: a filter group, plus "a value for either
/// `ApproveAfterDays` or `ApproveUntilDate`".
fn patch_rule(classification: &str) -> Value {
    json!({
        "PatchFilterGroup": {
            "PatchFilters": [{ "Key": "CLASSIFICATION", "Values": [classification] }],
        },
        "ApproveAfterDays": 1,
        "ComplianceLevel": "HIGH",
        "EnableNonSecurity": true,
    })
}

/// One approval rule carrying only what `PatchRule` requires plus the one member "your
/// request must include a value for" — so a read of it reports the default of every member
/// it left out.
fn bare_patch_rule(classification: &str) -> Value {
    json!({
        "PatchFilterGroup": {
            "PatchFilters": [{ "Key": "CLASSIFICATION", "Values": [classification] }],
        },
        "ApproveAfterDays": 7,
    })
}

/// A rule group of `count` rules, each selecting its own classification.
fn rule_group(count: usize) -> Value {
    // The classifications a Windows patch filter accepts, from `PatchFilter`'s own doc
    // (`smithy_query(query="doc", shape="PatchFilter")`).
    const CLASSIFICATIONS: &[&str] = &[
        "CriticalUpdates",
        "SecurityUpdates",
        "DefinitionUpdates",
        "Drivers",
        "FeaturePacks",
        "ServicePacks",
        "Tools",
        "UpdateRollups",
        "Updates",
        "Upgrades",
    ];
    json!({
        "PatchRules": (0..count)
            .map(|index| patch_rule(CLASSIFICATIONS[index % CLASSIFICATIONS.len()]))
            .collect::<Vec<Value>>(),
    })
}

/// `count` patch ids starting at `first`, in the `KB…` form a Windows patch id takes.
fn patch_ids(first: usize, count: usize) -> Value {
    json!((0..count)
        .map(|index| format!("KB{:07}", first + index))
        .collect::<Vec<String>>())
}

/// `count` patch sources, each with the three members `PatchSource` requires.
fn sources(count: usize) -> Value {
    json!((0..count)
        .map(|index| json!({
            "Name": format!("Repo-{index}"),
            "Products": [format!("Ubuntu22.{index:02}")],
            "Configuration": format!("deb http://repo-{index}.example.invalid/ubuntu jammy main"),
        }))
        .collect::<Vec<Value>>())
}

/// What a read of the baseline a create with `body` minted must report, member by member:
/// the members the create supplied, the documented default of each member it left out, and
/// `Null` — reported by nothing at all — for a member with neither.
fn expected_members(body: &Value) -> Map<String, Value> {
    let supplied = |member: &str| body.get(member).cloned().unwrap_or(Value::Null);
    let defaulted = |member: &str, default: Value| match body.get(member) {
        Some(value) => value.clone(),
        None => default,
    };
    let mut expected = Map::new();
    expected.insert("Name".to_string(), supplied("Name"));
    // A set of filters and a set of rules the write named none of are reported as the
    // EMPTY set, and every member of a rule the write left out at its own default.
    expected.insert(
        "GlobalFilters".to_string(),
        defaulted("GlobalFilters", json!({ "PatchFilters": [] })),
    );
    expected.insert(
        "ApprovalRules".to_string(),
        expected_approval_rules(body.get("ApprovalRules")),
    );
    expected.insert(
        "OperatingSystem".to_string(),
        defaulted("OperatingSystem", json!(DEFAULT_OPERATING_SYSTEM)),
    );
    expected.insert(
        "ApprovedPatches".to_string(),
        defaulted("ApprovedPatches", json!([])),
    );
    expected.insert(
        "ApprovedPatchesComplianceLevel".to_string(),
        defaulted(
            "ApprovedPatchesComplianceLevel",
            json!(DEFAULT_COMPLIANCE_LEVEL),
        ),
    );
    expected.insert(
        "ApprovedPatchesEnableNonSecurity".to_string(),
        defaulted("ApprovedPatchesEnableNonSecurity", json!(false)),
    );
    expected.insert(
        "RejectedPatches".to_string(),
        defaulted("RejectedPatches", json!([])),
    );
    expected.insert(
        "RejectedPatchesAction".to_string(),
        defaulted("RejectedPatchesAction", json!(DEFAULT_REJECTED_PATCHES_ACTION)),
    );
    expected.insert("Sources".to_string(), defaulted("Sources", json!([])));
    // A patch group is included in a baseline by registering it there, and this create
    // registered none.
    expected.insert("PatchGroups".to_string(), json!([]));
    // The members with no documented default: reported exactly when the create supplied
    // one, and reported by nothing when it did not.
    for member in ["Description", "AvailableSecurityUpdatesComplianceStatus"] {
        expected.insert(member.to_string(), supplied(member));
    }
    expected
}

/// The rule group a read reports for a create that supplied `supplied`: every rule as it
/// was written, with `ComplianceLevel` at the unspecified level and `EnableNonSecurity` at
/// false for a rule that named neither (`PatchRule$EnableNonSecurity`: "the default value
/// is false"), and the empty group when the create supplied none.
fn expected_approval_rules(supplied: Option<&Value>) -> Value {
    let rules = supplied
        .and_then(|rules| rules.get("PatchRules"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    json!({
        "PatchRules": rules
            .into_iter()
            .map(|rule| {
                let mut rule = rule;
                if rule.get("ComplianceLevel").is_none() {
                    rule["ComplianceLevel"] = json!(DEFAULT_COMPLIANCE_LEVEL);
                }
                if rule.get("EnableNonSecurity").is_none() {
                    rule["EnableNonSecurity"] = json!(false);
                }
                rule
            })
            .collect::<Vec<Value>>(),
    })
}

/// One row of the round-trip table: the body one create carries, at one Region.
struct RoundTripCase<'a> {
    case: &'a str,
    region: &'a str,
    body: Value,
}

/// A read reports every member the create persisted, and the documented default of every
/// member the create left out — whichever members the create carried, however many entries
/// they hold, for whichever operating system and at whichever Region.
#[tokio::test]
async fn a_read_reports_every_member_the_create_persisted() {
    let cases = [
        RoundTripCase {
            case: "only the members a create must carry",
            region: REGION,
            body: json!({
                "Name": "Minimal.Baseline",
                "ApprovedPatches": patch_ids(2_124_261, 1),
            }),
        },
        RoundTripCase {
            case: "every member a create may carry",
            region: REGION,
            body: json!({
                "Name": "Fully.Specified.Baseline",
                "OperatingSystem": "AMAZON_LINUX_2023",
                "Description": "a baseline whose every member is supplied",
                "GlobalFilters": {
                    "PatchFilters": [{ "Key": "PRODUCT", "Values": ["AmazonLinux2023"] }],
                },
                "ApprovalRules": rule_group(2),
                "ApprovedPatches": patch_ids(1_000_000, 3),
                "ApprovedPatchesComplianceLevel": "CRITICAL",
                "ApprovedPatchesEnableNonSecurity": true,
                "RejectedPatches": patch_ids(2_000_000, 2),
                "RejectedPatchesAction": "BLOCK",
                "Sources": sources(1),
                "AvailableSecurityUpdatesComplianceStatus": "NON_COMPLIANT",
                "Tags": [{ "Key": "Owner", "Value": "SysAdmin" }],
            }),
        },
        RoundTripCase {
            case: "approval by rule alone, at another Region",
            region: "eu-central-1",
            body: json!({
                "Name": "Rules.Only.Baseline",
                "OperatingSystem": "UBUNTU",
                "ApprovalRules": rule_group(1),
            }),
        },
        RoundTripCase {
            case: "every list filled to its published ceiling",
            region: "ap-southeast-1",
            body: json!({
                "Name": "Full.Lists.Baseline",
                "OperatingSystem": "MACOS",
                "ApprovedPatches": patch_ids(3_000_000, MAX_PATCH_IDS),
                "RejectedPatches": patch_ids(4_000_000, MAX_PATCH_IDS),
                "ApprovalRules": rule_group(MAX_PATCH_RULES),
                "Sources": sources(MAX_SOURCES),
            }),
        },
        RoundTripCase {
            case: "an empty approved-patch list beside one rule",
            region: REGION,
            body: json!({
                "Name": "Empty.List.Baseline",
                "OperatingSystem": "SUSE",
                "ApprovedPatches": [],
                "RejectedPatches": [],
                "Sources": [],
                "ApprovalRules": rule_group(1),
                "AvailableSecurityUpdatesComplianceStatus": "COMPLIANT",
            }),
        },
        RoundTripCase {
            case: "rules that name neither their level nor their non-security flag",
            region: REGION,
            body: json!({
                "Name": "Bare.Rules.Baseline",
                "OperatingSystem": "AMAZON_LINUX_2023",
                "ApprovalRules": {
                    "PatchRules": [
                        bare_patch_rule("Security"),
                        bare_patch_rule("Bugfix"),
                    ],
                },
            }),
        },
        RoundTripCase {
            case: "a description at its published floor",
            region: REGION,
            body: json!({
                "Name": "Short.Description.Baseline",
                "OperatingSystem": "WINDOWS",
                "Description": "a",
                "ApprovedPatches": patch_ids(5_000_000, 1),
            }),
        },
    ];

    for RoundTripCase { case, region, body } in &cases {
        let router = router();
        let baseline_id = create_ok(&router, case, region, body).await;
        let reported = get_ok(&router, case, region, &baseline_id).await;

        assert_eq!(
            reported["BaselineId"].as_str(),
            Some(baseline_id.as_str()),
            "case {case}: the read reports the id the create minted {baseline_id:?}, got \
             {reported}"
        );
        for (member, expected) in expected_members(body) {
            assert_eq!(
                reported[&member], expected,
                "case {case}: member {member} must be {expected}, got {}",
                reported[&member]
            );
        }

        // "The date the patch baseline was created" and "the date the patch baseline was
        // last modified": nothing has written the baseline since its create, so the two
        // are the one instant that create ran at.
        let created = reported["CreatedDate"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: the read reports a CreatedDate, got {reported}")
        });
        let modified = reported["ModifiedDate"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: the read reports a ModifiedDate, got {reported}")
        });
        assert_eq!(
            created, modified,
            "case {case}: an unmodified baseline was last written when it was created \
             ({created}), got {modified}"
        );
        assert!(
            created > 0.0,
            "case {case}: the created date is the instant the create ran, got {created}"
        );

        // A read writes nothing: a second read reports exactly what the first one did.
        let again = get_ok(&router, case, region, &baseline_id).await;
        assert_eq!(
            again, reported,
            "case {case}: a read must not change what a later read of {baseline_id:?} \
             reports, got {again} after {reported}"
        );
    }
}

/// A baseline the account created is read by its own id and by its full ARN alike: both
/// spellings report the same baseline, at whichever Region holds it.
#[tokio::test]
async fn either_spelling_of_a_held_baseline_reads_the_same_baseline() {
    let cases = [
        ("the endpoint's own Region", REGION, "WINDOWS"),
        ("another Region", "eu-central-1", "AMAZON_LINUX_2"),
        ("a third Region", "ap-northeast-1", "DEBIAN"),
    ];

    for (case, region, operating_system) in cases {
        let router = router();
        let body = json!({
            "Name": "Either.Spelling.Baseline",
            "OperatingSystem": operating_system,
            "ApprovedPatches": patch_ids(2_124_261, 2),
        });
        let baseline_id = create_ok(&router, case, region, &body).await;

        let by_id = get_ok(&router, case, region, &baseline_id).await;
        let by_arn = get_ok(&router, case, region, &baseline_arn(region, &baseline_id)).await;
        assert_eq!(
            by_arn, by_id,
            "case {case}: the ARN of {baseline_id:?} reads the same baseline its id does, \
             got {by_arn} and {by_id}"
        );
        assert_eq!(
            by_id["OperatingSystem"].as_str(),
            Some(operating_system),
            "case {case}: the read reports the operating system the create named, got {by_id}"
        );
    }
}

/// Every operating system's predefined baseline is readable by the full ARN a read of the
/// default reports — "to retrieve information about an Amazon Web Services managed patch
/// baseline, specify the full Amazon Resource Name (ARN) of the baseline" — and reports the
/// name Patch Manager gives it and the operating system it applies to.
#[tokio::test]
async fn a_predefined_baseline_is_read_by_the_arn_that_names_it() {
    let router = router();
    let mut reported_names: Vec<(&str, String)> = Vec::new();
    for operating_system in OPERATING_SYSTEMS {
        let case = *operating_system;
        let arn = predefined_baseline_id(&router, case, REGION, operating_system).await;
        let reported = get_ok(&router, case, REGION, &arn).await;
        assert_eq!(
            reported["BaselineId"].as_str(),
            Some(arn.as_str()),
            "case {case}: a predefined baseline is named by its full ARN {arn:?}, got \
             {reported}"
        );
        assert_eq!(
            reported["OperatingSystem"].as_str(),
            Some(*operating_system),
            "case {case}: the read reports the operating system the baseline applies to, \
             got {reported}"
        );
        let name = reported["Name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the read reports a Name, got {reported}"))
            .to_string();
        assert!(
            name.starts_with("AWS-") && name.ends_with("DefaultPatchBaseline"),
            "case {case}: a predefined baseline carries the name Patch Manager gives it \
             (AWS-…DefaultPatchBaseline), got {name:?}"
        );
        reported_names.push((operating_system, name));
    }

    for (case, name) in &reported_names {
        let collisions = reported_names
            .iter()
            .filter(|(_, held)| held == name)
            .count();
        assert_eq!(
            collisions, 1,
            "case {case}: each operating system has its own predefined baseline, but \
             {name:?} is reported for {collisions} of them"
        );
    }
}

/// A `BaselineId` that is well formed but names no baseline reachable through the endpoint
/// is refused as absent, with the status, the error class and the sentence Patch Manager
/// words — whichever way the id misses.
#[tokio::test]
async fn a_read_of_an_unheld_baseline_is_refused_as_absent() {
    let router = router();
    let held = create_ok(
        &router,
        "the held baseline",
        REGION,
        &json!({
            "Name": "Held.Baseline",
            "OperatingSystem": "SUSE",
            "ApprovedPatches": patch_ids(2_124_261, 1),
        }),
    )
    .await;
    // A baseline of ANOTHER Region's endpoint: a baseline is reached through the endpoint
    // of the one Region it was created in.
    let elsewhere = create_ok(
        &router,
        "the baseline of another Region",
        "eu-central-1",
        &json!({
            "Name": "Elsewhere.Baseline",
            "OperatingSystem": "SUSE",
            "ApprovedPatches": patch_ids(2_124_261, 1),
        }),
    )
    .await;

    // One hex digit of the held id changed, so the value is as well formed as the held one
    // and names nothing.
    let mut near_miss: Vec<char> = held.chars().collect();
    let last = near_miss.len() - 1;
    near_miss[last] = if near_miss[last] == 'f' { '0' } else { 'f' };
    let near_miss: String = near_miss.into_iter().collect();

    let cases: Vec<(&str, String)> = vec![
        ("an id no create minted", near_miss),
        ("a baseline of another Region", elsewhere.clone()),
        (
            "the ARN of a baseline of another Region, at this endpoint",
            baseline_arn(REGION, &elsewhere),
        ),
        (
            "an id at the published floor of the member",
            "pb-00000000000000000".to_string(),
        ),
        (
            "a name-like value no baseline carries",
            "AWS-DefaultPatchBaselineXXX".to_string(),
        ),
    ];
    for (case, baseline_id) in cases {
        let (status, response) = get(&router, REGION, &baseline_id).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DoesNotExistException"),
            "case {case}: expected 400 DoesNotExistException for {baseline_id:?}, got \
             {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            NOT_FOUND_MESSAGE,
            "case {case}: expected the sentence Patch Manager words, got {response}"
        );

        // The refused read changed nothing: the baseline this endpoint holds still reads.
        let reported = get_ok(&router, case, REGION, &held).await;
        assert_eq!(
            reported["Name"].as_str(),
            Some("Held.Baseline"),
            "case {case}: the held baseline still reads, got {reported}"
        );
    }
}

/// A `BaselineId` that could never name a baseline held at this endpoint — an ARN of
/// another service, of another account, of another Region, or of another resource type — is
/// refused as an invalid resource id rather than resolved.
#[tokio::test]
async fn a_read_of_an_id_this_endpoint_could_never_hold_is_refused_as_invalid() {
    let router = router();
    let held = create_ok(
        &router,
        "the held baseline",
        REGION,
        &json!({
            "Name": "Invalid.Id.Baseline",
            "OperatingSystem": "WINDOWS",
            "ApprovedPatches": patch_ids(2_124_261, 1),
        }),
    )
    .await;

    let cases: Vec<(&str, String)> = vec![
        (
            "an ARN of another account",
            format!("arn:aws:ssm:{REGION}:210987654321:patchbaseline/{held}"),
        ),
        (
            "an ARN of another Region",
            format!("arn:aws:ssm:eu-central-1:{ACCOUNT}:patchbaseline/{held}"),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:patchbaseline/{held}"),
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:ssm:{REGION}:{ACCOUNT}:document/{held}"),
        ),
    ];
    for (case, baseline_id) in cases {
        let (status, response) = get(&router, REGION, &baseline_id).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidResourceId"),
            "case {case}: expected 400 InvalidResourceId for {baseline_id:?}, got {status} \
             {response}"
        );

        let reported = get_ok(&router, case, REGION, &held).await;
        assert_eq!(
            reported["BaselineId"].as_str(),
            Some(held.as_str()),
            "case {case}: the held baseline still reads, got {reported}"
        );
    }
}

/// A `BaselineId` outside the published bounds of the member is refused by the boundary
/// walk before any lookup: below its floor, above its ceiling, and built from characters
/// its pattern does not admit.
#[tokio::test]
async fn a_baseline_id_outside_the_modelled_member_is_refused() {
    // The published bounds of `BaselineId` (`smithy_query(query="shape",
    // shape="BaselineId")`: `length 20..128`, pattern `^[a-zA-Z0-9_\-:/]{20,128}$`).
    const MIN: usize = 20;
    const MAX: usize = 128;

    let router = router();
    let cases: Vec<(&str, String)> = vec![
        ("one short of the floor", format!("pb-{}", "0".repeat(MIN - 4))),
        ("one past the ceiling", format!("pb-{}", "0".repeat(MAX - 2))),
        ("a character the pattern excludes", format!("pb-{}", "a.".repeat(MIN))),
    ];
    for (case, baseline_id) in cases {
        let (status, response) = get(&router, REGION, &baseline_id).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException for {baseline_id:?}, got \
             {status} {response}"
        );
    }

    // Both published bounds themselves are values the member allows, so they reach the
    // lookup and are answered as absent rather than as malformed.
    let at_bounds = [
        ("at the floor", "0".repeat(MIN - 3)),
        ("at the ceiling", "0".repeat(MAX - 3)),
    ];
    for (case, digits) in at_bounds {
        let baseline_id = format!("pb-{digits}");
        let (status, response) = get(&router, REGION, &baseline_id).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DoesNotExistException"),
            "case {case}: a {} character id the pattern admits reaches the lookup, got \
             {status} {response}",
            baseline_id.len()
        );
    }
}
