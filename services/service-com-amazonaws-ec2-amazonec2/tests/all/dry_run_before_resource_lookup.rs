// SPDX-License-Identifier: CC0-1.0

//! Where EC2 answers `DryRun` relative to its own checks, for the operations whose recorded
//! contract puts the flag BEFORE the resource the request names is looked up — driven over
//! the wire the service actually speaks (`ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness).
//!
//! The rule under test: `DryRun` "checks whether you have the required permissions for the
//! action, WITHOUT ACTUALLY MAKING THE REQUEST … If you have the required permissions, the
//! error response is DryRunOperation", so for these operations the flag is answered once the
//! request's own members have been READ and before any state is resolved. Three consequences,
//! each a table below:
//!
//! 1. a dry run whose members are readable is answered 412 `DryRunOperation` — even when it
//!    names a resource the account does not hold, and even when the request would have been
//!    refused by a check that reads state;
//! 2. a value whose FORM the operation cannot read is refused with that operation's own
//!    malformed-id code even under `DryRun`: there is nothing to look up, whatever the caller
//!    may do;
//! 3. without the flag the same request is refused on its own terms, naming the resource that
//!    could not be resolved — and no dry run leaves a trace in the store.
//!
//! The rows span the operations the rule holds for and, per operation, the kinds of input it
//! generalises over (an absent resource, a request its own checks would refuse, an unreadable
//! id), so no row is the specification. WHERE the flag sits is a fact of each operation — the
//! RESET of an address's attribute is recorded answering its own refusals first — so only the
//! operations recorded answering the flag early are driven here.

use service_com_amazonaws_ec2_amazonec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::document;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// EC2's answer to an authorized dry run, and the status it carries it on.
const DRY_RUN_CODE: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// A well-formed AMI id the account can see no image for.
const ABSENT_IMAGE: &str = "ami-0fedcba987654321f";

/// A well-formed launch template id the account holds no template for.
const ABSENT_LAUNCH_TEMPLATE: &str = "lt-0fedcba987654321f";

/// A well-formed Elastic IP allocation id the account holds no address for.
const ABSENT_ALLOCATION: &str = "eipalloc-0fedcba987654321f";

/// A well-formed instance id the account holds no instance for.
const ABSENT_INSTANCE: &str = "i-0fedcba987654321f";

/// A domain name a modify of an address's attribute may name.
const DOMAIN: &str = "mail.example.com";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of the dry-run table: the case, the action, and the request's own members.
type DryRunRow = (&'static str, &'static str, Params);

/// One row of a refusal table: the case, the action, the members, and the code and message
/// the refusal carries.
type RefusalRow = (&'static str, &'static str, Params, &'static str, String);

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Allocate one Elastic IP address and answer its allocation id.
async fn allocate(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = call(router, REGION, "AllocateAddress", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AllocateAddress, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "allocationId")
        .filter_map(|node| node.text().map(str::to_string))
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no allocation id in {body}"))
}

/// RULE 1: a readable dry run is answered `DryRunOperation` before any state is resolved.
#[tokio::test]
async fn a_readable_dry_run_is_answered_before_the_state_is_resolved() {
    // (case, action, the request's own members beside the flag)
    let rows: Vec<DryRunRow> = vec![
        (
            "a fast launch disable naming an image the account cannot see",
            "DisableFastLaunch",
            vec![p("ImageId", ABSENT_IMAGE)],
        ),
        (
            "a fast launch enable naming an image the account cannot see",
            "EnableFastLaunch",
            vec![p("ImageId", ABSENT_IMAGE)],
        ),
        (
            "a fast launch enable naming a launch template the account does not hold",
            "EnableFastLaunch",
            vec![
                p("ImageId", ABSENT_IMAGE),
                p("LaunchTemplate.LaunchTemplateId", ABSENT_LAUNCH_TEMPLATE),
                p("LaunchTemplate.Version", "1"),
            ],
        ),
        (
            "an address attribute modify naming an address the account does not hold",
            "ModifyAddressAttribute",
            vec![
                p("AllocationId", ABSENT_ALLOCATION),
                p("DomainName", DOMAIN),
            ],
        ),
        (
            "a SQL Server High Availability enable naming an absent instance",
            "EnableInstanceSqlHaStandbyDetections",
            vec![p("InstanceId.1", ABSENT_INSTANCE)],
        ),
        (
            "a SQL Server High Availability disable naming an absent instance",
            "DisableInstanceSqlHaStandbyDetections",
            vec![p("InstanceId.1", ABSENT_INSTANCE)],
        ),
        (
            "a Reachability Analyzer sharing enable, which names no resource at all",
            "EnableReachabilityAnalyzerOrganizationSharing",
            Vec::new(),
        ),
    ];

    for (case, action, members) in rows {
        let (router, engine) = emulator();
        let mut params = members.clone();
        params.push(p("DryRun", "true"));
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_CODE,
            "case {case:?}: expected {DRY_RUN_CODE}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            DRY_RUN_MESSAGE,
            "case {case:?}: expected EC2's own dry-run wording, got body {body}"
        );
        // Nothing the dry run could have written is there: the flag is answered before the
        // first read, so there is nothing to undo either.
        let state = engine.state::<Ec2State>();
        let store = state.lock();
        assert!(
            store.sql_ha_registrations.is_empty()
                && store.address_ptr_records.is_empty()
                && store.fast_launch_images.is_empty()
                && !store.reachability_analyzer_organization_sharing(),
            "case {case:?}: a dry run must leave the store exactly as it was"
        );
    }
}

/// RULE 1, continued: a dry run of a request one of the operation's OWN checks would have
/// refused is still answered `DryRunOperation`, the flag asking about the caller rather than
/// about the request's chances.
#[tokio::test]
async fn a_dry_run_is_answered_even_for_a_request_the_state_would_refuse() {
    let (router, engine) = emulator();
    let allocation_id = allocate(&router, "the address under modification").await;

    // A first modify leaves an update outstanding on the record, which is what a second
    // modify of the same address is refused for.
    let modify = |extra: Params| {
        let mut params = vec![
            p("AllocationId", allocation_id.clone()),
            p("DomainName", DOMAIN),
        ];
        params.extend(extra);
        params
    };
    let (status, body) = call(&router, REGION, "ModifyAddressAttribute", &modify(Vec::new())).await;
    assert_eq!(
        status, 200,
        "the first modify must be served, got {status} with body {body}"
    );

    // Rows differing along how many dry runs have already been answered: each one is about
    // the caller, so the outstanding update neither changes the answer nor is consumed by it.
    for attempt in 1..=3 {
        let case = format!("dry run {attempt} while an update is outstanding");
        let (status, body) = call(
            &router,
            REGION,
            "ModifyAddressAttribute",
            &modify(vec![p("DryRun", "true")]),
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_CODE,
            "case {case:?}: expected {DRY_RUN_CODE}, got body {body}"
        );
        assert!(
            engine
                .state::<Ec2State>()
                .lock()
                .address_ptr_update_is_pending(&allocation_id),
            "case {case:?}: the dry run must leave the outstanding update alone"
        );
    }

    // Without the flag the same request IS refused by that check, so the two answers are
    // about different questions.
    let (status, body) = call(&router, REGION, "ModifyAddressAttribute", &modify(Vec::new())).await;
    assert_eq!(
        status, 400,
        "case \"a second modify while one is outstanding\": expected HTTP 400, \
         got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "PendingRequestAlreadyExists",
        "case \"a second modify while one is outstanding\": got body {body}"
    );
}

/// RULE 2: a value whose form the operation cannot read is refused with that operation's own
/// malformed-id code even under `DryRun`.
#[tokio::test]
async fn an_unreadable_id_is_refused_even_under_a_dry_run() {
    // (case, action, the members beside the flag, the code and message the refusal carries)
    let rows: Vec<RefusalRow> = vec![
        (
            "a fast launch disable naming something that is not an AMI id",
            "DisableFastLaunch",
            vec![p("ImageId", "not-an-ami")],
            "InvalidAMIID.Malformed",
            "Invalid id: \"not-an-ami\"".to_string(),
        ),
        (
            "a fast launch enable naming something that is not an AMI id",
            "EnableFastLaunch",
            vec![p("ImageId", "not-an-ami")],
            "InvalidAMIID.Malformed",
            "Invalid id: \"not-an-ami\"".to_string(),
        ),
        (
            "a fast launch enable whose launch template version is neither a number nor an \
             alias",
            "EnableFastLaunch",
            vec![
                p("ImageId", ABSENT_IMAGE),
                p("LaunchTemplate.LaunchTemplateId", ABSENT_LAUNCH_TEMPLATE),
                p("LaunchTemplate.Version", "latest"),
            ],
            "InvalidParameterValue",
            "Invalid value 'latest' for version. Valid values are $Latest, $Default, \
             and version numbers"
                .to_string(),
        ),
        (
            "a fast launch enable naming its launch template by both of its keys",
            "EnableFastLaunch",
            vec![
                p("ImageId", ABSENT_IMAGE),
                p("LaunchTemplate.LaunchTemplateId", ABSENT_LAUNCH_TEMPLATE),
                p("LaunchTemplate.LaunchTemplateName", "no.such.template"),
                p("LaunchTemplate.Version", "1"),
            ],
            "InvalidParameterCombination",
            "Either the launch template ID or the launch template name may be specified, \
             but not both"
                .to_string(),
        ),
        (
            "an address attribute modify naming something that is not an allocation id",
            "ModifyAddressAttribute",
            vec![p("AllocationId", "not-an-allocation"), p("DomainName", DOMAIN)],
            "InvalidAllocationID.Malformed",
            "Invalid id: \"not-an-allocation\"".to_string(),
        ),
        (
            "a SQL Server High Availability enable naming the all-zero instance id",
            "EnableInstanceSqlHaStandbyDetections",
            vec![p("InstanceId.1", "i-00000000000000000")],
            "InvalidInstanceID.Malformed",
            "The instance ID 'i-00000000000000000' is malformed".to_string(),
        ),
        (
            "a SQL Server High Availability disable naming the all-zero instance id",
            "DisableInstanceSqlHaStandbyDetections",
            vec![p("InstanceId.1", "i-00000000000000000")],
            "InvalidInstanceID.Malformed",
            "The instance ID 'i-00000000000000000' is malformed".to_string(),
        ),
    ];

    for (case, action, members, code, message) in rows {
        let (router, _engine) = emulator();
        let mut params = members.clone();
        params.push(p("DryRun", "true"));
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 under the flag, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code} under the flag, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected that operation's own wording, got body {body}"
        );

        // The same value is refused the same way WITHOUT the flag: the form is a fact of the
        // request, not of the permission check.
        let (status, body) = call(&router, REGION, action, &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 without the flag, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code} without the flag, got body {body}"
        );
    }
}

/// RULE 3: without the flag, a request naming a resource the account does not hold is refused
/// for THAT reason, naming the resource it could not resolve.
#[tokio::test]
async fn without_the_flag_an_absent_resource_is_the_refusal() {
    // (case, action, members, the code and message the refusal carries)
    let rows: Vec<RefusalRow> = vec![
        (
            "a fast launch disable naming an image the account cannot see",
            "DisableFastLaunch",
            vec![p("ImageId", ABSENT_IMAGE)],
            "InvalidAMIID.NotFound",
            format!("The image id '[{ABSENT_IMAGE}]' does not exist"),
        ),
        (
            "a fast launch enable naming an image the account cannot see",
            "EnableFastLaunch",
            vec![p("ImageId", ABSENT_IMAGE)],
            "InvalidAMIID.NotFound",
            format!("The image id '[{ABSENT_IMAGE}]' does not exist"),
        ),
        (
            "an address attribute modify naming an address the account does not hold",
            "ModifyAddressAttribute",
            vec![
                p("AllocationId", ABSENT_ALLOCATION),
                p("DomainName", DOMAIN),
            ],
            "InvalidElasticIpID.NotFound",
            format!("The elasticIp ID '{ABSENT_ALLOCATION}' does not exist"),
        ),
        (
            "a SQL Server High Availability enable naming an absent instance",
            "EnableInstanceSqlHaStandbyDetections",
            vec![p("InstanceId.1", ABSENT_INSTANCE)],
            "InvalidInstanceID.NotFound",
            format!("The instance ID '{ABSENT_INSTANCE}' does not exist"),
        ),
    ];

    for (case, action, members, code, message) in rows {
        let (router, _engine) = emulator();
        let (status, body) = call(&router, REGION, action, &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected that operation's own wording, got body {body}"
        );
    }
}

/// The trust relationship `EnableReachabilityAnalyzerOrganizationSharing` establishes is a
/// FACT THE STORE HOLDS: a dry run leaves it as it was, a real call establishes it and answers
/// it as it then stands, and establishing one that already holds changes nothing.
#[tokio::test]
async fn establishing_the_reachability_analyzer_trust_relationship_is_recorded() {
    let (router, engine) = emulator();
    let sharing = || engine.state::<Ec2State>().lock().reachability_analyzer_organization_sharing();
    const ACTION: &str = "EnableReachabilityAnalyzerOrganizationSharing";

    assert!(
        !sharing(),
        "an organization that has never established the relationship must not hold one"
    );

    let (status, body) = call(&router, REGION, ACTION, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "case \"a dry run\": expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    assert!(
        !sharing(),
        "case \"a dry run\": the relationship must not have been established"
    );

    // Rows differing along how many times the relationship has already been established: the
    // answer is read off the store, so a repeat reports the same thing and changes nothing.
    for attempt in 1..=3 {
        let case = format!("establishing it, attempt {attempt}");
        let extra = if attempt == 3 {
            // `DryRun=false` is not a dry run: it is an ordinary call.
            vec![p("DryRun", "false")]
        } else {
            Vec::new()
        };
        let (status, body) = call(&router, REGION, ACTION, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let parsed = document(&case, &body);
        let returned = parsed
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "returnValue")
            .filter_map(|node| node.text().map(str::to_string))
            .next();
        assert_eq!(
            returned.as_deref(),
            Some("true"),
            "case {case:?}: expected the relationship to be reported as established, \
             got body {body}"
        );
        assert!(
            sharing(),
            "case {case:?}: the store must hold the relationship the answer reported"
        );
    }
}
