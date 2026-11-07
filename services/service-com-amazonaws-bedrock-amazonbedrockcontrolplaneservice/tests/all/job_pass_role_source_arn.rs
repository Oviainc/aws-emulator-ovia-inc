// SPDX-License-Identifier: CC0-1.0

//! The CONDITION CONTEXT a job create's `iam:PassRole` decision is taken under, driven over
//! the real wire path (a restJson1 request through the multi-service router into this
//! service mounted beside IAM and STS on one engine), so what is asserted is what a client
//! sees.
//!
//! THE RULE UNDER TEST: Amazon Bedrock takes a passed role ON for the job the create mints,
//! so the pass is decided under the context that assume carries — the caller's account as
//! `aws:SourceAccount` and, where the ARN of the job being created is fixed at the point the
//! decision is taken, that job's ARN as `aws:SourceArn` — and a trust statement narrowed
//! over a key the context holds a value for is DECIDED (it admits the pass where it holds
//! and withholds it where it does not), while one narrowed over a key the context holds no
//! value for leaves the statement undecided, which is no grant.
//!
//! The table spans what the rule generalises over:
//!
//! * the OPERATION, hence the job family the pass is made for: `CreateModelInvocationJob`
//!   (`model-invocation-job`), `CreateEvaluationJob` (`evaluation-job`) and
//!   `CreateModelCustomizationJob`, whose `ModelCustomizationJobArn` spells the resolved base
//!   model before the job id — a segment the decision stands in front of — so that create
//!   states no source ARN at all;
//! * the OPERATOR the clause is written under (`ArnLike`, `ArnEquals`, `StringLike` and the
//!   negated `ArnNotLike`), which is what decides whether a scope that matches admits or
//!   withholds;
//! * the SCOPE the clause names: the family of the job being created, another job family of
//!   this service, one named job of the family, every Bedrock resource of the account,
//!   another account's jobs and another Region's;
//! * and, for the admitted rows, the job that was minted, read back through the operation's
//!   own read.
//!
//! Every expectation is derived from the model
//! (`smithy_query(query="operation",operation="CreateModelInvocationJob,CreateEvaluationJob,CreateModelCustomizationJob,GetModelInvocationJob,GetEvaluationJob")`:
//! the job ARN patterns, the success statuses and `AccessDeniedException` at
//! `@httpError(403)`) or from the published condition-key semantics IAM applies to a trust
//! policy.
//!
//! The emulator, the signed call and the role fixture are the harnesses
//! `crate::custom_model_authorization` and `crate::control_plane_authorization` own, reused
//! rather than copied: one rule, one dispatch path.

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::control_plane_authorization::{
    evaluation_job_body, given_role, invocation_job_body, trust, JOB_ROLE,
};
use crate::custom_model_authorization::{emulator, Emulator, ACCOUNT_KEY_ID};
use crate::support::{label, Reply, ACCOUNT, REGION};

/// The sentence Amazon Bedrock states a denied pass-role with, spelled here so this suite
/// pins the wording a client sees rather than reading it off the service.
const PASS_ROLE_REFUSAL: &str = "Cross-account pass role is not allowed.";

/// The published foundation model catalog this service serves, so the base model the
/// customization rows carry follows the catalog rather than a pinned spelling.
const CATALOG: &[bedrock::state::FoundationModel] = bedrock::state::FOUNDATION_MODELS;

/// The customization a create that names none runs, as the operation documents it
/// ("Creates a fine-tuning job to customize a base model").
const DEFAULT_CUSTOMIZATION: &str = "FINE_TUNING";

/// The statuses a batch inference job that has just been submitted may be reported in: the
/// non-terminal ones of `ModelInvocationJobStatus`.
const INVOCATION_NON_TERMINAL: &[&str] = &[
    bedrock::state::MODEL_INVOCATION_JOB_STATUS_SUBMITTED,
    bedrock::state::MODEL_INVOCATION_JOB_STATUS_VALIDATING,
    bedrock::state::MODEL_INVOCATION_JOB_STATUS_SCHEDULED,
    bedrock::state::MODEL_INVOCATION_JOB_STATUS_IN_PROGRESS,
];

/// The status an evaluation job that has just been created is reported in.
const EVALUATION_NON_TERMINAL: &[&str] = &[bedrock::state::EVALUATION_JOB_STATUS_IN_PROGRESS];

/// A base model the catalog publishes as supporting `FINE_TUNING`: the identifier a caller
/// that read `ListFoundationModels` would submit to a customization create.
fn fine_tuning_model() -> &'static str {
    CATALOG
        .iter()
        .find(|model| model.customizations_supported.contains(&DEFAULT_CUSTOMIZATION))
        .map(|model| model.model_id)
        .expect("the catalog must publish a base model supporting FINE_TUNING")
}

/// A `CreateModelCustomizationJob` body carrying the members the model marks required, with
/// the custom model named after the job so two rows never collide.
fn customization_job_body(name: &str, role_arn: &str) -> Value {
    json!({
        "jobName": name,
        "customModelName": name,
        "baseModelIdentifier": fine_tuning_model(),
        "roleArn": role_arn,
        "trainingDataConfig": { "s3Uri": "s3://customization-data/train.jsonl" },
        "outputDataConfig": { "s3Uri": "s3://customization-output/" },
    })
}

/// The ARN of the `family` resources `tail` names in this account and Region, written the
/// way this service writes the ARNs of the jobs it mints.
fn job_arn(family: &str, tail: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:{family}/{tail}")
}

/// One job-creating operation: the create, the family of the job it mints, whether the pass
/// it makes states a source ARN at all, and how the minted job is read back.
struct JobCreate {
    case: &'static str,
    path: &'static str,
    body: fn(&str, &str) -> Value,
    /// The success status the model declares for the create.
    success: u16,
    /// The resource type of the job's own ARN.
    family: &'static str,
    /// Whether the ARN of the job being created is fixed where the pass is decided, hence
    /// stated as `aws:SourceArn`. A customization job's ARN carries the resolved base model
    /// before its id, and that resolution happens after the decision.
    states_source_arn: bool,
    /// The read that reports the minted job, addressed by its ARN.
    read: fn(&str) -> String,
    /// The statuses the read may report for a job this create just minted.
    non_terminal: &'static [&'static str],
    /// The operation's own listing filtered by `nameContains`, where one is implemented:
    /// what a refused create must leave empty.
    listing: Option<(&'static str, &'static str)>,
}

/// Every job-creating operation of this service that passes a role the job RUNS as.
fn job_creates() -> Vec<JobCreate> {
    vec![
        JobCreate {
            case: "CreateModelInvocationJob",
            path: "/model-invocation-job",
            body: invocation_job_body,
            success: 200,
            family: "model-invocation-job",
            states_source_arn: true,
            read: |arn| format!("/model-invocation-job/{}", label(arn)),
            non_terminal: INVOCATION_NON_TERMINAL,
            listing: Some(("/model-invocation-jobs", "invocationJobSummaries")),
        },
        JobCreate {
            case: "CreateEvaluationJob",
            path: "/evaluation-jobs",
            body: evaluation_job_body,
            success: 202,
            family: "evaluation-job",
            states_source_arn: true,
            read: |arn| format!("/evaluation-jobs/{}", label(arn)),
            non_terminal: EVALUATION_NON_TERMINAL,
            // `ListEvaluationJobs` is a scaffolded 501 under its own issue, so there is no
            // listing to read a refused create's absence out of here.
            listing: None,
        },
        JobCreate {
            case: "CreateModelCustomizationJob",
            path: "/model-customization-jobs",
            body: customization_job_body,
            success: 201,
            family: "model-customization-job",
            states_source_arn: false,
            read: |arn| format!("/model-customization-jobs/{}", label(arn)),
            non_terminal: &[bedrock::state::MODEL_CUSTOMIZATION_JOB_STATUS_IN_PROGRESS],
            listing: Some((
                "/model-customization-jobs",
                "modelCustomizationJobSummaries",
            )),
        },
    ]
}

/// One trust-policy condition, over the job family it is SCOPED to.
struct Clause {
    case: &'static str,
    /// The `Condition` element, built over the family the clause is scoped to.
    condition: fn(&str) -> Value,
    /// Whether a pass that states the ARN of a job of THAT family is admitted.
    on_own_family: bool,
    /// Whether a pass that states the ARN of a job of ANOTHER family is admitted.
    on_other_family: bool,
}

/// The clauses a trust policy narrows the assume with, each decided against the source ARN
/// the pass carries.
const CLAUSES: &[Clause] = &[
    Clause {
        // The two confused-deputy clauses AWS documents a Bedrock batch-inference service
        // role's trust policy with.
        case: "the documented pair: this account, and the family's jobs under ArnLike",
        condition: |family| {
            json!({
                "StringEquals": { "aws:SourceAccount": ACCOUNT },
                "ArnLike": { "aws:SourceArn": job_arn(family, "*") },
            })
        },
        on_own_family: true,
        on_other_family: false,
    },
    Clause {
        // IAM documents `ArnEquals` and `ArnLike` as performing the same comparison, so the
        // same wildcard scope decides alike under either.
        case: "the family's jobs under ArnEquals",
        condition: |family| json!({ "ArnEquals": { "aws:SourceArn": job_arn(family, "*") } }),
        on_own_family: true,
        on_other_family: false,
    },
    Clause {
        // The same key under a STRING operator: a condition key carrying an ARN is still a
        // string, and the wildcard its `…Like` spelling admits is the same one.
        case: "the family's jobs under StringLike",
        condition: |family| json!({ "StringLike": { "aws:SourceArn": job_arn(family, "*") } }),
        on_own_family: true,
        on_other_family: false,
    },
    Clause {
        // A negated operator holds exactly where its positive sibling does not, so a pass
        // whose source ARN IS of the family it excludes is withheld and one of another
        // family is granted.
        case: "every job but the family's, under ArnNotLike",
        condition: |family| json!({ "ArnNotLike": { "aws:SourceArn": job_arn(family, "*") } }),
        on_own_family: false,
        on_other_family: true,
    },
    Clause {
        // ONE job of the family, named by an id of the family's own published form
        // (`[a-z0-9]{12}`) that is not the id this create mints: the clause is decided
        // against the ARN of the job being created, which is some other job.
        case: "one named job of the family under ArnEquals",
        condition: |family| {
            json!({ "ArnEquals": { "aws:SourceArn": job_arn(family, "aaaaaaaaaaaa") } })
        },
        on_own_family: false,
        on_other_family: false,
    },
    Clause {
        // Every Bedrock resource of this account in this Region, which covers the job
        // whichever family it belongs to.
        case: "every Bedrock resource of this account under ArnLike",
        condition: |_| {
            json!({
                "ArnLike": { "aws:SourceArn": format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:*") },
            })
        },
        on_own_family: true,
        on_other_family: true,
    },
    Clause {
        case: "the family's jobs in ANOTHER account",
        condition: |family| {
            json!({
                "ArnLike": {
                    "aws:SourceArn": format!("arn:aws:bedrock:{REGION}:123456789012:{family}/*"),
                },
            })
        },
        on_own_family: false,
        on_other_family: false,
    },
    Clause {
        case: "the family's jobs in ANOTHER Region",
        condition: |family| {
            json!({
                "ArnLike": {
                    "aws:SourceArn": format!("arn:aws:bedrock:eu-west-1:{ACCOUNT}:{family}/*"),
                },
            })
        },
        on_own_family: false,
        on_other_family: false,
    },
];

/// The families a clause is scoped to: the one each create mints a job of, and one of this
/// service's jobs no create under test mints.
const SCOPES: &[&str] = &[
    "model-invocation-job",
    "evaluation-job",
    "advanced-prompt-optimization-job",
];

/// Whether `arn` is a minted job ARN of `family` in this account and Region, as the job ARN
/// members publish one (`…:<family>/[a-z0-9]{12}`).
fn is_minted_job_arn(arn: &str, family: &str) -> bool {
    arn.strip_prefix(&format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:{family}/"))
        .is_some_and(|id| {
            id.len() == 12
                && id
                    .chars()
                    .all(|glyph| glyph.is_ascii_lowercase() || glyph.is_ascii_digit())
        })
}

/// Whether this account holds a job of `name` for `create`, read through the operation's own
/// listing where one is implemented.
async fn job_listed(emu: &Emulator, create: &JobCreate, case: &str, name: &str) -> bool {
    let Some((listing, member)) = create.listing else {
        return false;
    };
    let reply = emu
        .call_as(
            ACCOUNT_KEY_ID,
            "GET",
            &format!("{listing}?nameContains={name}"),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: the listing must be served, got {} {:?}",
        reply.status, reply.body
    );
    match reply.get(member) {
        Value::Null => false,
        Value::Array(rows) => !rows.is_empty(),
        other => panic!("case {case}: {member} must be a list, got {other}"),
    }
}

/// Assert `reply` is the modelled pass-role refusal, whole.
fn assert_refused(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 403,
        "case {case}: a pass that cannot be shown to be granted must be refused 403, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("AccessDeniedException"),
        "case {case}: expected AccessDeniedException, got {:?}",
        reply.error
    );
    assert_eq!(
        reply.get("message"),
        json!(PASS_ROLE_REFUSAL),
        "case {case}: expected {PASS_ROLE_REFUSAL:?}, got {:?}",
        reply.body
    );
}

/// RULE: a job create's pass is decided under the context the assume carries — this account,
/// and the ARN of the job being created where that ARN is fixed at the decision point — so a
/// trust statement narrowed over `aws:SourceArn` is decided against that ARN, and admits the
/// pass exactly where the scope it names covers the job.
#[tokio::test]
async fn a_job_creates_pass_is_decided_against_the_source_arn_of_the_job_it_mints() {
    let emu = emulator();
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{JOB_ROLE}");

    for (create_index, create) in job_creates().iter().enumerate() {
        for (clause_index, clause) in CLAUSES.iter().enumerate() {
            for (scope_index, scope) in SCOPES.iter().enumerate() {
                let condition = (clause.condition)(scope);
                given_role(&emu.engine, JOB_ROLE, Some(&trust(Some(condition.clone()))));

                // A pass that states no source ARN at all cannot satisfy a clause over that
                // key, however the clause is scoped: the statement stays undecided, which is
                // no grant.
                let admitted = if create.states_source_arn {
                    if *scope == create.family {
                        clause.on_own_family
                    } else {
                        clause.on_other_family
                    }
                } else {
                    false
                };
                let name = format!("src{create_index}-{clause_index}-{scope_index}");
                let case = format!(
                    "{} / {:?} scoped to {scope:?}",
                    create.case, clause.case
                );
                let reply = emu
                    .call_as(
                        ACCOUNT_KEY_ID,
                        "POST",
                        create.path,
                        Some((create.body)(&name, &role_arn)),
                    )
                    .await;

                if !admitted {
                    assert_refused(&case, &reply);
                    assert!(
                        !job_listed(&emu, create, &case, &name).await,
                        "case {case}: a refused pass must leave no job behind"
                    );
                    continue;
                }

                assert_eq!(
                    reply.status, create.success,
                    "case {case}: a pass the grant covers must be admitted, got {} {:?}",
                    reply.status, reply.body
                );
                let arn = reply.text("jobArn");
                assert!(
                    is_minted_job_arn(&arn, create.family),
                    "case {case}: expected a minted {} ARN, got {arn:?}",
                    create.family
                );

                // The job the pass was decided for is the job the create minted: the read
                // reports it, in a status it has just been put in.
                let read = emu
                    .call_as(ACCOUNT_KEY_ID, "GET", &(create.read)(&arn), None)
                    .await;
                assert_eq!(
                    read.status, 200,
                    "case {case}: the minted job must be readable back, got {} {:?}",
                    read.status, read.body
                );
                let status = read.text("status");
                assert!(
                    create.non_terminal.contains(&status.as_str()),
                    "case {case}: expected one of {:?}, got {status:?}",
                    create.non_terminal
                );
            }
        }
    }
}

/// RULE, the other half: the account key alone still decides the pass, so a trust policy
/// narrowed only by `aws:SourceAccount` admits every one of these creates — which is what
/// makes a refusal above attributable to the source-ARN clause and to nothing else.
#[tokio::test]
async fn the_account_clause_alone_admits_every_job_create() {
    let emu = emulator();
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{JOB_ROLE}");

    // (case, the condition the trust is narrowed by, whether the pass is admitted)
    let rows: [(&str, Option<Value>, bool); 3] = [
        ("no condition at all", None, true),
        (
            "this account under StringEquals",
            Some(json!({ "StringEquals": { "aws:SourceAccount": ACCOUNT } })),
            true,
        ),
        (
            "another account under StringEquals",
            Some(json!({ "StringEquals": { "aws:SourceAccount": "123456789012" } })),
            false,
        ),
    ];

    for (row_index, (row_case, condition, admitted)) in rows.into_iter().enumerate() {
        given_role(&emu.engine, JOB_ROLE, Some(&trust(condition)));
        for (create_index, create) in job_creates().iter().enumerate() {
            let name = format!("acct{row_index}-{create_index}");
            let case = format!("{} / {row_case:?}", create.case);
            let reply = emu
                .call_as(
                    ACCOUNT_KEY_ID,
                    "POST",
                    create.path,
                    Some((create.body)(&name, &role_arn)),
                )
                .await;
            if admitted {
                assert_eq!(
                    reply.status, create.success,
                    "case {case}: a pass of this account's own role must be admitted, got {} {:?}",
                    reply.status, reply.body
                );
                continue;
            }
            assert_refused(&case, &reply);
        }
    }
}
