// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ValidateSecurityGroupQuotasForInterface`, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rules under test are:
//!
//! 1. AN OPERATION THE SERVICE PERMITS ONLY TO AUTHORIZED AMAZON WEB SERVICES SERVICES
//!    REFUSES EVERY OTHER CALLER with `UnauthorizedOperation`, HTTP 403, carrying the fixed
//!    sentence that error class answers for such an operation — and reports no `Valid`
//!    verdict. "Only authorized Amazon Web Services services can call this operation"
//!    (`ValidateSecurityGroupQuotasForInterface`), and every caller of this emulator signs
//!    as a principal of the account rather than as one of those services.
//! 2. THAT ANSWER DOES NOT DEPEND ON THE GROUPS THE REQUEST NAMES, because the decision is
//!    about who is calling: it is the same whether the request names one security group of
//!    the account, as many as a network interface may hold, one more than that, the same
//!    group twice, an id the account holds no group for, a value that is no group id at all,
//!    or a group of the account beside one it holds nothing for. A `DryRun` request is
//!    answered AHEAD of it by the permission probe that flag is — "if you have the required
//!    permissions, the error response is DryRunOperation" (`$DryRun`) — so it reports 412
//!    `DryRunOperation` for the caller of this emulator, which holds the account's actions.
//!    A request that names NO group is the one case the operation never sees:
//!    `SecurityGroupIds` is a required member, so the boundary refuses it before any handler
//!    runs.
//! 3. NEITHER ANSWER PERSISTS ANYTHING OR CHANGES WHAT A LATER CALLER SEES: the groups,
//!    their rules and the bytes the store has charged are exactly what they were before the
//!    call, and `DescribeSecurityGroups` reports what it reported before — so the same
//!    request asked twice is answered twice the same way.
//!
//! Each table row is a distinct input along what the rules generalise over — how many groups
//! the request names, whether they exist, whether naming them all on one interface would
//! pass the documented quota, and whether the request is a dry run — so no row is the
//! specification.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, MAX_SECURITY_GROUPS_PER_INSTANCE};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message};
use super::volume_lifecycle::{REGION, p};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A well-formed security group id of the long form EC2 mints, which nothing here created,
/// for the row about an id the account holds no group for.
const UNKNOWN_GROUP_ID: &str = "sg-0123456789abcdef0";

/// A value that is no security group id at all, for the row about one.
const NOT_A_GROUP_ID: &str = "not-a-group";

/// The code, status and message EC2 answers a caller it does not serve this operation to:
/// `UnauthorizedOperation`, HTTP 403, with that error class's fixed sentence for an operation
/// only authorized Amazon Web Services services may call. Spelled out here rather than
/// imported from the implementation, so a change to either one is what this suite catches.
const UNAUTHORIZED_OPERATION: &str = "UnauthorizedOperation";
const UNAUTHORIZED_STATUS: u16 = 403;
const CALLER_NOT_ALLOWED_MESSAGE: &str = "Caller is not allowed to call this API.";

/// The code and status EC2 answers a `DryRun` request with for a caller that holds the
/// action: 412 `DryRunOperation`, with the fixed sentence that probe is answered with.
const DRY_RUN_OPERATION: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// `SecurityGroupId.N` form fields, as ec2Query flattens the request's one list into
/// indexed keys. Takes owned ids because every id a row names is one a create minted.
fn id_params(ids: &[String]) -> Params {
    ids.iter()
        .enumerate()
        .map(|(index, id)| p(&format!("SecurityGroupId.{}", index + 1), id.clone()))
        .collect()
}

/// Create `count` security groups in the Region's default VPC, through `CreateSecurityGroup`,
/// and answer the ids it minted — so every id a row names is one this account really holds.
async fn create_groups(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let params = vec![
            p("GroupName", format!("interface-quota-{index}")),
            p("GroupDescription", format!("group {index} of a network interface")),
        ];
        let (status, body) = op(router, "CreateSecurityGroup", &params).await;
        assert_eq!(
            status, 200,
            "CreateSecurityGroup {index} must succeed, got {status}: {body}"
        );
        let group_id = element(&body, "groupId")
            .unwrap_or_else(|| panic!("CreateSecurityGroup {index} must report a groupId: {body}"));
        ids.push(group_id);
    }
    ids
}

/// Every security group `DescribeSecurityGroups` reports, as the `(id, name)` pairs a later
/// caller reads them back as — the reporting half's own view of what this call must leave
/// alone. The envelope's `requestId` is minted per response, so the groups are compared
/// rather than the whole body.
async fn described_groups(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let (status, body) = op(router, "DescribeSecurityGroups", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeSecurityGroups must succeed, got {status}: {body}"
    );
    elements(&body, "groupId")
        .into_iter()
        .zip(elements(&body, "groupName"))
        .collect()
}

/// Everything the store holds that this call must leave alone: the groups, the rules they
/// hold and the caller-supplied bytes charged for them.
fn stored(engine: &engine::InMemoryEngine) -> (usize, usize, usize) {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    (
        guard.security_groups.len(),
        guard.security_group_rules.len(),
        guard.stored_bytes(),
    )
}

/// Assert that `status`/`body` is the answer this operation gives a caller of this emulator:
/// the probe's `DryRunOperation` for a request carrying `DryRun=true`, and otherwise the
/// `UnauthorizedOperation` refusal of a caller the API does not serve. Neither answer reports
/// a `Valid` verdict.
fn assert_answered(case: &str, status: u16, body: &str, dry_run: Option<bool>) {
    let (expected_status, expected_code, expected_message) = if dry_run == Some(true) {
        (DRY_RUN_STATUS, DRY_RUN_OPERATION, DRY_RUN_MESSAGE)
    } else {
        (
            UNAUTHORIZED_STATUS,
            UNAUTHORIZED_OPERATION,
            CALLER_NOT_ALLOWED_MESSAGE,
        )
    };
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
    );
    let code = error_code(body);
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected {expected_code}, got {code:?} in {body}"
    );
    let message = error_message(body);
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {expected_message:?}, got {message:?}"
    );
    assert!(
        element(body, "valid").is_none() && element(body, "Valid").is_none(),
        "case {case:?}: neither answer reports a verdict, got {body}"
    );
}

/// `(case, the ids the request names, the `DryRun` flag it carries)` for one row, resolved
/// against the groups this account holds.
fn rows(held: &[String]) -> Vec<(String, Vec<String>, Option<bool>)> {
    // `MAX_SECURITY_GROUPS_PER_INSTANCE` is this crate's name for AWS's documented maximum
    // number of security groups per network interface — the quota this operation is about —
    // so the table spans it: one below nothing, at the maximum, and one past it.
    let at_the_maximum = &held[..MAX_SECURITY_GROUPS_PER_INSTANCE];
    let past_the_maximum = &held[..MAX_SECURITY_GROUPS_PER_INSTANCE + 1];
    vec![
        (
            "one security group the account holds".to_string(),
            vec![held[0].clone()],
            None,
        ),
        (
            "two security groups the account holds".to_string(),
            vec![held[0].clone(), held[1].clone()],
            None,
        ),
        (
            format!("as many groups as one network interface may hold ({MAX_SECURITY_GROUPS_PER_INSTANCE})"),
            at_the_maximum.to_vec(),
            None,
        ),
        (
            format!("one group more than that ({})", MAX_SECURITY_GROUPS_PER_INSTANCE + 1),
            past_the_maximum.to_vec(),
            None,
        ),
        (
            "the same group named twice".to_string(),
            vec![held[0].clone(), held[0].clone()],
            None,
        ),
        (
            "a well-formed id the account holds no group for".to_string(),
            vec![UNKNOWN_GROUP_ID.to_string()],
            None,
        ),
        (
            "a value that is no security group id at all".to_string(),
            vec![NOT_A_GROUP_ID.to_string()],
            None,
        ),
        (
            "a group of the account beside one it holds nothing for".to_string(),
            vec![held[0].clone(), UNKNOWN_GROUP_ID.to_string()],
            None,
        ),
        (
            "one group of the account, as a dry run".to_string(),
            vec![held[0].clone()],
            Some(true),
        ),
        (
            format!(
                "one group past the per-interface maximum ({}), as a dry run",
                MAX_SECURITY_GROUPS_PER_INSTANCE + 1
            ),
            past_the_maximum.to_vec(),
            Some(true),
        ),
        (
            "one group of the account, with the dry run flag set false".to_string(),
            vec![held[0].clone()],
            Some(false),
        ),
    ]
}

/// The parameters of one row: the ids it names, and its `DryRun` flag when it carries one.
fn row_params(ids: &[String], dry_run: Option<bool>) -> Params {
    let mut params = id_params(ids);
    if let Some(flag) = dry_run {
        params.push(p("DryRun", if flag { "true" } else { "false" }));
    }
    params
}

// ---------------------------------------------------------------------------
// RULES 1 and 2: the answer is decided by the caller, not by the groups named
// ---------------------------------------------------------------------------

/// Every request a caller of this emulator can make is answered by WHO is calling: the quota
/// question is never reached, so neither the number of groups named, their existence nor
/// their uniqueness changes the answer, and only the `DryRun` flag does — by asking the
/// permission question instead.
#[tokio::test]
async fn the_answer_is_decided_by_the_caller_not_by_the_groups_named() {
    let (router, engine) = emulator();
    let held = create_groups(&router, MAX_SECURITY_GROUPS_PER_INSTANCE + 1).await;

    for (case, ids, dry_run) in rows(&held) {
        let params = row_params(&ids, dry_run);
        let (status, body) = op(
            &router,
            "ValidateSecurityGroupQuotasForInterface",
            &params,
        )
        .await;
        assert_answered(&case, status, &body, dry_run);
    }

    // The store must have nothing of its own to say about those answers: the groups the
    // creates above minted are all it holds.
    let (groups, rules, _bytes) = stored(&engine);
    assert!(
        groups >= held.len(),
        "the groups the creates minted must still be held, got {groups} of at least {}",
        held.len()
    );
    assert!(
        rules > 0,
        "each created group holds the documented allow-all outbound rule, got {rules} rules"
    );
}

/// A request that names no security group never reaches the operation at all: the member is
/// required (`ValidateSecurityGroupQuotasForInterfaceRequest$SecurityGroupIds`), so the
/// boundary refuses it as a malformed request — which is a different claim from either answer
/// above, and also stores nothing.
#[tokio::test]
async fn a_request_naming_no_security_group_is_refused_at_the_boundary() {
    let (router, engine) = emulator();
    let baseline = stored(&engine);

    let (status, body) = op(&router, "ValidateSecurityGroupQuotasForInterface", &[]).await;
    assert_eq!(
        status, 400,
        "a request naming no security group: expected HTTP 400, got {status} with body {body}"
    );
    assert!(
        error_message(&body).contains("securityGroupIds"),
        "a request naming no security group: expected the refusal to name the member it \
         requires, got {body}"
    );
    assert!(
        element(&body, "valid").is_none() && element(&body, "Valid").is_none(),
        "a request naming no security group reports no verdict, got {body}"
    );
    assert_eq!(
        stored(&engine),
        baseline,
        "the boundary refusal stores nothing, got {:?} where the store started at \
         {baseline:?}",
        stored(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 3: neither answer persists anything, and both repeat themselves
// ---------------------------------------------------------------------------

/// Neither answer writes anything or changes what a later caller sees, so the same request
/// asked twice is answered twice the same way.
#[tokio::test]
async fn neither_answer_persists_anything_and_both_repeat_themselves() {
    let (router, engine) = emulator();
    let held = create_groups(&router, MAX_SECURITY_GROUPS_PER_INSTANCE + 1).await;

    let baseline = stored(&engine);
    let described = described_groups(&router).await;

    for (case, ids, dry_run) in rows(&held) {
        let params = row_params(&ids, dry_run);

        let mut answers = Vec::new();
        for attempt in 0..2 {
            let (status, body) = op(
                &router,
                "ValidateSecurityGroupQuotasForInterface",
                &params,
            )
            .await;
            assert_answered(&format!("{case} (attempt {attempt})"), status, &body, dry_run);
            answers.push((error_code(&body), error_message(&body)));
        }
        assert_eq!(
            answers[0], answers[1],
            "case {case:?}: the same request is answered the same way twice, got {answers:?}"
        );

        assert_eq!(
            stored(&engine),
            baseline,
            "case {case:?}: the call leaves the store as it found it, got {:?} where it \
             started at {baseline:?}",
            stored(&engine)
        );
    }

    assert_eq!(
        described_groups(&router).await,
        described,
        "neither answer changes what a later caller sees"
    );
}
