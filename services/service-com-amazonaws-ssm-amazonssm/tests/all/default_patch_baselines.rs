// SPDX-License-Identifier: CC0-1.0

//! The default patch baseline of an operating system, end to end: `CreatePatchBaseline`
//! mints a baseline the account owns, `RegisterDefaultPatchBaseline` records one as the
//! default for the operating system it applies to, and `GetDefaultPatchBaseline` reports
//! what is recorded.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`]) dispatches
//! `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router.
//! Nothing reaches into the state struct, so a response that is not computed from
//! persisted state cannot pass: every read follows a write.
//!
//! The rule under test is that a read reports, for the operating system the request names
//! — Windows when it names none — the baseline a register recorded for THAT operating
//! system at the endpoint's Region, and the baseline Systems Manager predefines for it
//! while nothing has registered one. The suites are tables whose rows differ along what
//! the rule generalises over: the operating system (every value of the modelled enum,
//! not only the one a recorded unit used), whether a baseline is the account's own or one
//! Systems Manager predefined, which spelling names it (bare id or full ARN), and the
//! Region its endpoint is.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, ACCOUNT, REGION};

/// The `OperatingSystem` values the model's enum declares
/// (`smithy_query(query="shape", shape="OperatingSystem")`), which is also the set
/// `crate::ops::types::OPERATING_SYSTEM_VALUES` holds.
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

/// The operating system a request that names none is about: "If you don't specify an
/// operating system value, the default patch baseline for Windows is returned"
/// (`smithy_query(query="doc", shape="GetDefaultPatchBaseline")`).
const DEFAULT_OPERATING_SYSTEM: &str = "WINDOWS";

/// The published floor of `BaselineId`
/// (`smithy_query(query="shape", shape="BaselineId")`: `length 20..128`,
/// pattern `^[a-zA-Z0-9_\-:/]{20,128}$`).
const BASELINE_ID_MIN: usize = 20;

/// The published ceiling of the same member.
const BASELINE_ID_MAX: usize = 128;

/// The sentence `DoesNotExistException` carries for a `BaselineId` that names no patch
/// baseline reachable through the endpoint: fixed text for this error class, naming the
/// resource class and interpolating nothing from the request. Spelled out here rather than
/// read from the crate, so the wording itself is what the assertions check.
const NOT_FOUND_MESSAGE: &str = "Patch Baseline to be retrieved does not exist.";

/// How many default patch baselines the store records at once
/// (`crate::state::patch_baselines::MAX_DEFAULT_PATCH_BASELINES`): the emulator's own
/// guard on the one map a register grows, not a quota AWS publishes.
const MAX_DEFAULT_PATCH_BASELINES: usize =
    service_com_amazonaws_ssm_amazonssm::state::patch_baselines::MAX_DEFAULT_PATCH_BASELINES;

/// `GetDefaultPatchBaseline` at `region`'s endpoint, for `operating_system` when a row
/// names one and for no operating system at all when it does not, returning
/// `(status, body)`.
async fn get_default(
    router: &MultiServiceRouter,
    region: &str,
    operating_system: Option<&str>,
) -> (u16, Value) {
    let body = match operating_system {
        Some(operating_system) => json!({ "OperatingSystem": operating_system }),
        None => json!({}),
    };
    call_in_region(router, region, "GetDefaultPatchBaseline", &body).await
}

/// The same read, asserting it succeeded, and returning `(BaselineId, OperatingSystem)`.
async fn get_default_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operating_system: Option<&str>,
) -> (String, String) {
    let (status, response) = get_default(router, region, operating_system).await;
    assert_eq!(
        status, 200,
        "case {case}: GetDefaultPatchBaseline must succeed, got {status} {response}"
    );
    let baseline_id = response["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a read reports a BaselineId, got {response}"))
        .to_string();
    let reported_os = response["OperatingSystem"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a read reports an OperatingSystem, got {response}"))
        .to_string();
    assert_within_modelled_baseline_id(case, &baseline_id);
    (baseline_id, reported_os)
}

/// Assert `baseline_id` is a value the modelled `BaselineId` allows: within its published
/// length bounds and built from the characters its pattern admits.
fn assert_within_modelled_baseline_id(case: &str, baseline_id: &str) {
    assert!(
        (BASELINE_ID_MIN..=BASELINE_ID_MAX).contains(&baseline_id.len()),
        "case {case}: a BaselineId is {BASELINE_ID_MIN}..{BASELINE_ID_MAX} characters, got \
         {baseline_id:?} of {}",
        baseline_id.len()
    );
    assert!(
        baseline_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '/')),
        "case {case}: a BaselineId holds only the characters its pattern admits, got \
         {baseline_id:?}"
    );
}

/// `CreatePatchBaseline` at `region`'s endpoint with `body`, returning `(status, body)`.
async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "CreatePatchBaseline", body).await
}

/// A create of a baseline named `name` for `operating_system`, asserting it succeeded,
/// and returning the `BaselineId` it minted.
async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    operating_system: &str,
) -> String {
    let (status, response) = create(
        router,
        region,
        &json!({
            "Name": name,
            "OperatingSystem": operating_system,
            "Description": format!("the baseline {case} registers"),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: CreatePatchBaseline must succeed, got {status} {response}"
    );
    let baseline_id = response["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a create reports a BaselineId, got {response}"))
        .to_string();
    assert_within_modelled_baseline_id(case, &baseline_id);
    baseline_id
}

/// `RegisterDefaultPatchBaseline` of `baseline_id` at `region`'s endpoint, returning
/// `(status, body)`.
async fn register(
    router: &MultiServiceRouter,
    region: &str,
    baseline_id: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "RegisterDefaultPatchBaseline",
        &json!({ "BaselineId": baseline_id }),
    )
    .await
}

/// The same write, asserting it succeeded, and returning the `BaselineId` it reports.
async fn register_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    baseline_id: &str,
) -> String {
    let (status, response) = register(router, region, baseline_id).await;
    assert_eq!(
        status, 200,
        "case {case}: RegisterDefaultPatchBaseline must succeed, got {status} {response}"
    );
    response["BaselineId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a register reports a BaselineId, got {response}"))
        .to_string()
}

/// The ARN of the patch baseline `baseline_id` names in `region`, the form the account's
/// own ARNs carry.
fn baseline_arn(region: &str, baseline_id: &str) -> String {
    format!("arn:aws:ssm:{region}:{ACCOUNT}:patchbaseline/{baseline_id}")
}

/// Every operating system the model declares has a default baseline before anything is
/// created or registered: the one Systems Manager predefines for it, named by its full
/// ARN — the spelling a register of a predefined baseline takes — reported for the
/// operating system that was asked for, stable across reads, and distinct per operating
/// system.
#[tokio::test]
async fn a_fresh_account_reports_the_predefined_default_of_every_operating_system() {
    let router = router();
    let mut reported: Vec<(&str, String)> = Vec::new();
    for operating_system in OPERATING_SYSTEMS {
        let case = *operating_system;
        let (baseline_id, reported_os) =
            get_default_ok(&router, case, REGION, Some(operating_system)).await;
        assert_eq!(
            reported_os, *operating_system,
            "case {case}: the read reports the operating system it was asked for, got \
             {reported_os:?}"
        );
        let prefix = baseline_arn(REGION, "pb-");
        assert!(
            baseline_id.starts_with(&prefix),
            "case {case}: a predefined baseline is named by its full ARN {prefix}…, got \
             {baseline_id:?}"
        );

        // A read writes nothing, so a second read of the same operating system reports
        // exactly what the first one did.
        let (again, again_os) = get_default_ok(&router, case, REGION, Some(operating_system)).await;
        assert_eq!(
            (again.as_str(), again_os.as_str()),
            (baseline_id.as_str(), reported_os.as_str()),
            "case {case}: a read reports the same default every time, got {again:?} {again_os:?}"
        );
        reported.push((case, baseline_id));
    }

    for (case, baseline_id) in &reported {
        let collisions = reported
            .iter()
            .filter(|(_, held)| held == baseline_id)
            .count();
        assert_eq!(
            collisions, 1,
            "case {case}: each operating system has its own predefined baseline, but \
             {baseline_id:?} is reported for {collisions} of them"
        );
    }
}

/// A request that names no operating system is a request about Windows: it reports
/// exactly what naming `WINDOWS` reports, before and after a register moves that default.
#[tokio::test]
async fn a_read_that_names_no_operating_system_is_about_windows() {
    let router = router();
    let predefined = get_default_ok(&router, "a fresh account, named WINDOWS", REGION, Some(DEFAULT_OPERATING_SYSTEM)).await;
    let unnamed = get_default_ok(&router, "a fresh account, naming nothing", REGION, None).await;
    assert_eq!(
        unnamed, predefined,
        "case a fresh account: naming no operating system reports the Windows default \
         {predefined:?}, got {unnamed:?}"
    );

    let created = create_ok(
        &router,
        "the Windows baseline",
        REGION,
        "Windows.Registered.Baseline",
        DEFAULT_OPERATING_SYSTEM,
    )
    .await;
    register_ok(&router, "the Windows baseline", REGION, &created).await;

    let named = get_default_ok(&router, "after a register, named WINDOWS", REGION, Some(DEFAULT_OPERATING_SYSTEM)).await;
    let unnamed = get_default_ok(&router, "after a register, naming nothing", REGION, None).await;
    assert_eq!(
        named,
        (created.clone(), DEFAULT_OPERATING_SYSTEM.to_string()),
        "case after a register: the Windows default is the registered baseline {created:?}, \
         got {named:?}"
    );
    assert_eq!(
        unnamed, named,
        "case after a register: naming no operating system reports the Windows default \
         {named:?}, got {unnamed:?}"
    );
}

/// A register records the default of the operating system the REGISTERED baseline applies
/// to: the read of that operating system reports the baseline the create minted, and
/// every other operating system still reports the baseline Systems Manager predefines for
/// it.
#[tokio::test]
async fn a_register_moves_the_default_of_its_own_operating_system_only() {
    let router = router();
    // What every operating system reports before anything is registered.
    let mut predefined: Vec<(&str, String)> = Vec::new();
    for operating_system in OPERATING_SYSTEMS {
        let (baseline_id, _) =
            get_default_ok(&router, operating_system, REGION, Some(operating_system)).await;
        predefined.push((operating_system, baseline_id));
    }

    for (nth, operating_system) in OPERATING_SYSTEMS.iter().enumerate() {
        let case = *operating_system;
        let created = create_ok(
            &router,
            case,
            REGION,
            &format!("Baseline-{nth}"),
            operating_system,
        )
        .await;
        let registered = register_ok(&router, case, REGION, &created).await;
        assert_eq!(
            registered, created,
            "case {case}: a register reports the baseline it recorded {created:?}, got \
             {registered:?}"
        );

        let (reported, reported_os) =
            get_default_ok(&router, case, REGION, Some(operating_system)).await;
        assert_eq!(
            (reported.as_str(), reported_os.as_str()),
            (created.as_str(), *operating_system),
            "case {case}: the default is the registered baseline {created:?} of \
             {operating_system}, got {reported:?} {reported_os:?}"
        );

        // Every operating system whose default nothing has registered yet still reports
        // its predefined baseline, and the ones already registered still report theirs.
        for (other_nth, (other, held)) in predefined.iter().enumerate() {
            if other_nth <= nth {
                continue;
            }
            let (reported, _) = get_default_ok(&router, other, REGION, Some(other)).await;
            assert_eq!(
                reported, *held,
                "case {case}: the {other} default is untouched at {held:?}, got {reported:?}"
            );
        }
    }
}

/// A later register replaces the one before it, and the full ARN a read reports for a
/// predefined baseline registers it back — "to reset the Amazon Web Services-predefined
/// patch baseline as the default, specify the full patch baseline Amazon Resource Name
/// (ARN) as the baseline ID value".
#[tokio::test]
async fn a_register_replaces_the_default_and_the_predefined_arn_resets_it() {
    let router = router();
    let operating_system = "AMAZON_LINUX_2";
    let case = "register, re-register, reset";
    let (predefined, _) = get_default_ok(&router, case, REGION, Some(operating_system)).await;

    let first = create_ok(&router, case, REGION, "First.Baseline", operating_system).await;
    let second = create_ok(&router, case, REGION, "Second.Baseline", operating_system).await;
    assert_ne!(
        first, second,
        "case {case}: two creates mint two baselines, got {first:?} twice"
    );

    for (round, expected) in [&first, &second, &first].into_iter().enumerate() {
        register_ok(&router, case, REGION, expected).await;
        let (reported, _) = get_default_ok(&router, case, REGION, Some(operating_system)).await;
        assert_eq!(
            reported, *expected,
            "case {case} round {round}: the default is the baseline last registered \
             {expected:?}, got {reported:?}"
        );
    }

    // The ARN the fresh account reported is a baseline this endpoint still holds, so
    // registering it puts the predefined baseline back.
    let registered = register_ok(&router, case, REGION, &predefined).await;
    assert_eq!(
        registered, predefined,
        "case {case}: resetting reports the predefined baseline {predefined:?}, got \
         {registered:?}"
    );
    let (reported, reported_os) = get_default_ok(&router, case, REGION, Some(operating_system)).await;
    assert_eq!(
        (reported.as_str(), reported_os.as_str()),
        (predefined.as_str(), operating_system),
        "case {case}: the default is the predefined baseline again, got {reported:?} \
         {reported_os:?}"
    );
}

/// A baseline the account created is reachable by its own id and by its full ARN alike,
/// and both spellings register the same baseline.
#[tokio::test]
async fn either_spelling_of_a_baseline_registers_the_same_baseline() {
    let router = router();
    let operating_system = "UBUNTU";
    let case = "the bare id and the full ARN";
    let created = create_ok(&router, case, REGION, "Spelling.Baseline", operating_system).await;

    for (spelling, value) in [
        ("the bare id", created.clone()),
        ("the full ARN", baseline_arn(REGION, &created)),
    ] {
        let registered = register_ok(&router, spelling, REGION, &value).await;
        assert_eq!(
            registered, created,
            "case {spelling}: a register names the baseline by its own id {created:?}, got \
             {registered:?}"
        );
        let (reported, _) = get_default_ok(&router, spelling, REGION, Some(operating_system)).await;
        assert_eq!(
            reported, created,
            "case {spelling}: the default is the created baseline {created:?}, got \
             {reported:?}"
        );
    }
}

/// Systems Manager is regional: the default is held per Region, so a register at one
/// endpoint leaves every other Region's default where it was, each Region predefines its
/// own baselines, and a baseline created at one endpoint names nothing at another.
#[tokio::test]
async fn the_default_is_held_per_region() {
    let router = router();
    let operating_system = "REDHAT_ENTERPRISE_LINUX";
    let elsewhere = "eu-central-1";
    let case = "two Regions";

    let (here, _) = get_default_ok(&router, case, REGION, Some(operating_system)).await;
    let (there, _) = get_default_ok(&router, case, elsewhere, Some(operating_system)).await;
    assert_ne!(
        here, there,
        "case {case}: each Region predefines its own baseline, got {here:?} at both"
    );
    assert!(
        there.starts_with(&baseline_arn(elsewhere, "pb-")),
        "case {case}: the other Region's baseline ARN names that Region, got {there:?}"
    );

    let created = create_ok(&router, case, REGION, "Regional.Baseline", operating_system).await;
    register_ok(&router, case, REGION, &created).await;

    let (reported_there, _) = get_default_ok(&router, case, elsewhere, Some(operating_system)).await;
    assert_eq!(
        reported_there, there,
        "case {case}: a register at one endpoint leaves {elsewhere}'s default at {there:?}, \
         got {reported_there:?}"
    );

    // The baseline is held at the Region it was created in, so its id names nothing at
    // the other endpoint and no default moves there.
    let (status, response) = register(&router, elsewhere, &created).await;
    assert_does_not_exist(case, status, &response);
    let (reported_there, _) = get_default_ok(&router, case, elsewhere, Some(operating_system)).await;
    assert_eq!(
        reported_there, there,
        "case {case}: the refused register wrote nothing at {elsewhere}, got {reported_there:?}"
    );
}

/// Assert `response` is the refusal a `BaselineId` naming no baseline carries: HTTP 400,
/// `DoesNotExistException`, and the one sentence this service words for it.
fn assert_does_not_exist(case: &str, status: u16, response: &Value) {
    assert_eq!(
        (status, error_code(response).as_str()),
        (400, "DoesNotExistException"),
        "case {case}: expected 400 DoesNotExistException, got {status} {response}"
    );
    assert_eq!(
        response["Message"].as_str(),
        Some(NOT_FOUND_MESSAGE),
        "case {case}: expected the message {NOT_FOUND_MESSAGE:?}, got {response}"
    );
}

/// A register naming a well-formed `BaselineId` the endpoint holds no baseline for is
/// refused with `DoesNotExistException`, writes nothing, and leaves the default where it
/// was — whichever way the id misses.
#[tokio::test]
async fn a_register_of_an_unheld_baseline_is_refused_and_writes_nothing() {
    let router = router();
    let operating_system = "SUSE";
    let held = create_ok(&router, "the held baseline", REGION, "Held.Baseline", operating_system).await;
    register_ok(&router, "the held baseline", REGION, &held).await;

    let (predefined_windows, _) =
        get_default_ok(&router, "the Windows default", REGION, Some(DEFAULT_OPERATING_SYSTEM)).await;
    // One hex digit of the held id changed, so the value is as well formed as the held one
    // and names nothing.
    let mut near_miss: Vec<char> = held.chars().collect();
    let last = near_miss.len() - 1;
    near_miss[last] = if near_miss[last] == 'f' { '0' } else { 'f' };
    let near_miss: String = near_miss.into_iter().collect();

    let cases: Vec<(&str, String)> = vec![
        ("an all-zero id", format!("pb-{}", "0".repeat(17))),
        ("an id nothing minted", "pb-7f3b2a194c5d4e6f8".to_string()),
        ("the held id with one digit changed", near_miss),
        (
            "the ARN of an id nothing minted",
            baseline_arn(REGION, &format!("pb-{}", "1".repeat(17))),
        ),
        (
            "a name that is not an id at all",
            "AWS-DefaultPatchBaselineXXX".to_string(),
        ),
    ];
    for (case, baseline_id) in cases {
        let (status, response) = register(&router, REGION, &baseline_id).await;
        assert_does_not_exist(case, status, &response);

        // Nothing was written: the default of the registered operating system is still
        // the baseline that WAS registered, and Windows still reports its predefined one.
        let (reported, _) = get_default_ok(&router, case, REGION, Some(operating_system)).await;
        assert_eq!(
            reported, held,
            "case {case}: a refused register leaves the {operating_system} default at \
             {held:?}, got {reported:?}"
        );
        let (windows, _) =
            get_default_ok(&router, case, REGION, Some(DEFAULT_OPERATING_SYSTEM)).await;
        assert_eq!(
            windows, predefined_windows,
            "case {case}: a refused register leaves the Windows default at \
             {predefined_windows:?}, got {windows:?}"
        );
    }
}

/// A `BaselineId` that could never name a baseline held at this endpoint — an ARN of
/// another service, of another account, of another Region, or of another resource type —
/// is refused as an invalid resource id rather than resolved, and writes nothing.
#[tokio::test]
async fn a_register_of_an_unresolvable_arn_is_refused_as_an_invalid_id() {
    let router = router();
    let operating_system = "DEBIAN";
    let held = create_ok(
        &router,
        "the held baseline",
        REGION,
        "Arn.Guard.Baseline",
        operating_system,
    )
    .await;
    register_ok(&router, "the held baseline", REGION, &held).await;

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
        let (status, response) = register(&router, REGION, &baseline_id).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidResourceId"),
            "case {case}: expected 400 InvalidResourceId, got {status} {response}"
        );
        let (reported, _) = get_default_ok(&router, case, REGION, Some(operating_system)).await;
        assert_eq!(
            reported, held,
            "case {case}: a refused register leaves the default at {held:?}, got {reported:?}"
        );
    }
}

/// A request outside a MODELLED constraint — a `BaselineId` shorter or longer than the
/// published bounds, an operating system outside the enum — is refused as a
/// `ValidationException` before any handler resolves anything, and changes nothing.
#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let operating_system = "CENTOS";
    let held = create_ok(
        &router,
        "the held baseline",
        REGION,
        "Modelled.Guard.Baseline",
        operating_system,
    )
    .await;
    register_ok(&router, "the held baseline", REGION, &held).await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "an id one character below the published floor",
            "RegisterDefaultPatchBaseline",
            json!({ "BaselineId": "p".repeat(BASELINE_ID_MIN - 1) }),
        ),
        (
            "an id one character past the published ceiling",
            "RegisterDefaultPatchBaseline",
            json!({ "BaselineId": "p".repeat(BASELINE_ID_MAX + 1) }),
        ),
        (
            "an id holding a character the pattern forbids",
            "RegisterDefaultPatchBaseline",
            json!({ "BaselineId": format!("pb-{}", "*".repeat(17)) }),
        ),
        (
            "no id at all",
            "RegisterDefaultPatchBaseline",
            json!({}),
        ),
        (
            "an operating system outside the modelled enum",
            "GetDefaultPatchBaseline",
            json!({ "OperatingSystem": "PLAN9" }),
        ),
        (
            "an empty operating system",
            "GetDefaultPatchBaseline",
            json!({ "OperatingSystem": "" }),
        ),
    ];
    for (case, operation, body) in cases {
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException from {operation}, got {status} \
             {response}"
        );
        let (reported, _) = get_default_ok(&router, case, REGION, Some(operating_system)).await;
        assert_eq!(
            reported, held,
            "case {case}: a refused request leaves the default at {held:?}, got {reported:?}"
        );
    }
}

/// The store a register writes to is bounded: the Region a default is recorded under comes
/// from the request's own credential scope, so once the store records as many defaults as
/// the emulator keeps, a register that would record a NEW `(Region, operating system)` is
/// refused and writes nothing, while a default already held still reads and still
/// re-registers.
#[tokio::test]
async fn the_store_a_register_writes_to_is_bounded() {
    let router = router();
    let regions = MAX_DEFAULT_PATCH_BASELINES / OPERATING_SYSTEMS.len();
    let held_region = "fill-region-0";

    // In the first Region, the Windows default is a baseline the account CREATED, so the
    // entry held there is distinguishable from the predefined baseline a read falls back
    // to — which is what makes "nothing was written" checkable below.
    let held = create_ok(
        &router,
        "the held default",
        held_region,
        "Bounded.Store.Baseline",
        DEFAULT_OPERATING_SYSTEM,
    )
    .await;
    register_ok(&router, "the held default", held_region, &held).await;

    // Fill the rest of the store by registering, in each Region, the baseline Systems
    // Manager predefines for each operating system — the reference a read of that Region
    // reports, so no create is needed to reach the ceiling.
    for nth in 0..regions {
        let region = format!("fill-region-{nth}");
        for operating_system in OPERATING_SYSTEMS {
            if region == held_region && *operating_system == DEFAULT_OPERATING_SYSTEM {
                continue;
            }
            let case = format!("filling {region} {operating_system}");
            let (predefined, _) =
                get_default_ok(&router, &case, &region, Some(operating_system)).await;
            register_ok(&router, &case, &region, &predefined).await;
        }
    }

    // One Region past the ceiling: a baseline created there registers nowhere, because
    // recording its default would be a new key in a full store.
    let case = "one past the ceiling";
    let beyond = format!("fill-region-{regions}");
    let (predefined_beyond, _) =
        get_default_ok(&router, case, &beyond, Some(DEFAULT_OPERATING_SYSTEM)).await;
    let created_beyond = create_ok(
        &router,
        case,
        &beyond,
        "Past.Ceiling.Baseline",
        DEFAULT_OPERATING_SYSTEM,
    )
    .await;
    let (status, response) = register(&router, &beyond, &created_beyond).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case {case}: expected the refusal this crate's other total guards answer with, got \
         {status} {response}"
    );
    let refusal = response["Message"]
        .as_str()
        .or_else(|| response["message"].as_str())
        .unwrap_or_default();
    assert!(
        refusal.contains(&MAX_DEFAULT_PATCH_BASELINES.to_string()),
        "case {case}: the refusal must name the total it tripped, got {response}"
    );

    // Nothing was written: that Region's Windows default is still the predefined baseline,
    // not the one the refused register named.
    let (reported, _) = get_default_ok(&router, case, &beyond, Some(DEFAULT_OPERATING_SYSTEM)).await;
    assert_eq!(
        reported, predefined_beyond,
        "case {case}: a refused register leaves {beyond}'s default at the predefined \
         {predefined_beyond:?}, got {reported:?}"
    );

    // A default the store already holds still reads, and still takes a register: the guard
    // refuses a NEW key, so it cannot wedge an operating system whose default is set.
    let (reported, _) =
        get_default_ok(&router, "the held default", held_region, Some(DEFAULT_OPERATING_SYSTEM))
            .await;
    assert_eq!(
        reported, held,
        "case the held default: the full store still reports {held:?}, got {reported:?}"
    );
    let registered = register_ok(&router, "the held default", held_region, &held).await;
    assert_eq!(
        registered, held,
        "case the held default: a register of a default already held still succeeds, got \
         {registered:?}"
    );
}
#[tokio::test]
async fn a_create_is_idempotent_under_one_client_token() {
    let router = router();
    let operating_system = "ORACLE_LINUX";
    let body = json!({
        "Name": "Token.Baseline",
        "OperatingSystem": operating_system,
        "Description": "the baseline a retry reports",
        "ClientToken": "0d8f2a1c-1111-2222-3333-444455556666",
    });

    let (status, first) = create(&router, REGION, &body).await;
    assert_eq!(
        status, 200,
        "case the first create: CreatePatchBaseline must succeed, got {status} {first}"
    );
    let minted = first["BaselineId"]
        .as_str()
        .expect("case the first create: a create reports a BaselineId")
        .to_string();

    let (status, retried) = create(&router, REGION, &body).await;
    assert_eq!(
        (status, retried["BaselineId"].as_str()),
        (200, Some(minted.as_str())),
        "case the retry: a retry under one token reports {minted:?}, got {status} {retried}"
    );

    // The same token carrying different members is another call, not a retry.
    let mut mismatched = body.clone();
    mismatched["Description"] = json!("a different baseline under the same token");
    let (status, response) = create(&router, REGION, &mismatched).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "IdempotentParameterMismatch"),
        "case the mismatch: expected 400 IdempotentParameterMismatch, got {status} {response}"
    );

    // Two creates that carry NO token are two baselines, however alike their members.
    let one = create_ok(&router, "no token, first", REGION, "No.Token.Baseline", operating_system).await;
    let two = create_ok(&router, "no token, second", REGION, "No.Token.Baseline", operating_system).await;
    assert_ne!(
        one, two,
        "case no token: two creates mint two baselines, got {one:?} twice"
    );

    // Each of them registers in turn, and the default is whichever was registered last.
    register_ok(&router, "no token, first", REGION, &one).await;
    register_ok(&router, "no token, second", REGION, &two).await;
    let (reported, _) = get_default_ok(&router, "no token", REGION, Some(operating_system)).await;
    assert_eq!(
        reported, two,
        "case no token: the default is the baseline registered last {two:?}, got {reported:?}"
    );
}
