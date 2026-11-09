// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for how SageMaker WORDS the refusal of a resource the account
//! does not hold, on the task-governance (cluster policy, compute quota), JumpStart hub,
//! model-card, model-package, partner-AI-app, Inference-Recommender and HyperPod-cluster
//! surfaces.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via the
//! harness `inference_deployment_lifecycle` declares, so what these tests exercise is what
//! a real SDK client gets.
//!
//! Each test asserts ONE sentence over a TABLE of inputs rather than one recorded call:
//! the status, the error code and the WHOLE message, whose fixed text is the service's and
//! whose variable part is the identifier taken from the request and resolved against the
//! Region the request signed for. Every expected sentence is spelled out here rather than
//! read from the constant under test, so the wire form is what is pinned. The rows differ
//! along what each rule generalises over: the identifier's form (name, ARN, another Region,
//! another account), its shape (shortest, longest, digit-leading, mixed case), the
//! operation that resolves it, and — where the resource is versioned — a version it never
//! reached.
//!
//! The success paths of these resources are the business of their own suites; what each
//! test adds beside its refusals is the control that the resource the account DOES hold is
//! still served, so a refusal cannot come from an empty store.

use serde_json::{json, Value};

use crate::cluster_scheduler_config_lifecycle::{create as create_policy, seed_cluster};
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The one ARN form this service mints:
/// `arn:aws:sagemaker:<region>:<account>:<kind>/<resource>`. Spelled out here so these
/// tests pin the wire form rather than echoing the helper under test.
fn sagemaker_arn(region: &str, kind: &str, resource: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{kind}/{resource}")
}

/// Assert `refused` is the modelled refusal carrying exactly `expected`.
fn assert_refusal(case: &str, status: u16, refused: &Value, code: &str, expected: &str) {
    assert_eq!(
        (status, error_code(refused).as_str()),
        (400, code),
        "case {case}: expected 400 {code}, got {status} with {refused:?}"
    );
    assert_eq!(
        message(refused),
        expected,
        "case {case}: expected the message {expected:?}, got {:?}",
        message(refused)
    );
}

// ---------------------------------------------------------------------------
// HyperPod task governance: the cluster policy and the compute quota, both
// addressed by the id SageMaker minted and both reported by the ARN it resolves to.
// ---------------------------------------------------------------------------

/// Ids of the shape `ClusterSchedulerConfigId` / `ComputeQuotaId` fix (`^[a-z0-9]{12}$`,
/// smithy_query operation=DescribeClusterSchedulerConfig) that nothing ever minted: the
/// two extremes of the alphabet and one mixed value.
const UNMINTED_IDS: [(&str, &str); 3] = [
    ("the lowest id of the modelled shape", "000000000000"),
    ("the highest id of the modelled shape", "ffffffffffff"),
    ("a mixed id", "a1b2c3d4e5f6"),
];

/// Versions no freshly created resource is at: `CreateClusterSchedulerConfig` and
/// `CreateComputeQuota` mint version 1, so one ahead of it and the two values below it are
/// versions the resource never reached.
const UNREACHED_VERSIONS: [i32; 3] = [2, 0, -1];

/// RULE: every operation that cannot resolve the cluster policy a request names refuses it
/// with the modelled `ResourceNotFound` and the message
/// `Cluster scheduler config with arn <arn> could not be found` — the ARN the requested
/// `ClusterSchedulerConfigId` resolves to in the Region the request signed for, unquoted
/// and with no trailing period. A version the policy never reached is the same miss.
#[tokio::test]
async fn an_unknown_cluster_policy_is_refused_naming_its_resolved_arn() {
    // The fixture (and so the stored policy) lives in the Region the harness signs for by
    // default; the rows below sign for others too, because the ARN a refusal names is
    // resolved against the Region the REQUEST signed for.
    let home = "us-east-1";
    let emulator = Emulator::new();
    let cluster = seed_cluster(&emulator, home, "wording-cluster").await;
    let held = create_policy(&emulator, "wording-policy", &cluster).await;

    // Rows: an id nothing minted, in every signed Region.
    for region in [home, "eu-west-1", "ap-south-1"] {
        for (case, id) in UNMINTED_IDS {
            let case = format!("{region}/{case}");
            let (status, refused) = emulator
                .call_in(
                    region,
                    "DescribeClusterSchedulerConfig",
                    &json!({ "ClusterSchedulerConfigId": id }),
                )
                .await;
            let expected = format!(
                "Cluster scheduler config with arn {} could not be found",
                sagemaker_arn(region, "cluster-scheduler-config", id)
            );
            assert_refusal(&case, status, &refused, "ResourceNotFound", &expected);
            assert!(
                !message(&refused).ends_with('.'),
                "case {case}: the sentence carries no trailing period, got {:?}",
                message(&refused)
            );
        }
    }

    // And a version the held policy never reached is the same miss, named by the policy's
    // own ARN.
    let expected = format!(
        "Cluster scheduler config with arn {} could not be found",
        sagemaker_arn(home, "cluster-scheduler-config", &held)
    );
    for version in UNREACHED_VERSIONS {
        let (status, refused) = emulator
            .call_in(
                home,
                "DescribeClusterSchedulerConfig",
                &json!({
                    "ClusterSchedulerConfigId": held,
                    "ClusterSchedulerConfigVersion": version,
                }),
            )
            .await;
        assert_refusal(
            &format!("a version the policy never reached: {version}"),
            status,
            &refused,
            "ResourceNotFound",
            &expected,
        );
    }

    // Control: the policy the account holds, at the version it is at, is served.
    let described = emulator
        .call_in(
            home,
            "DescribeClusterSchedulerConfig",
            &json!({ "ClusterSchedulerConfigId": held, "ClusterSchedulerConfigVersion": 1 }),
        )
        .await;
    assert_eq!(
        str_field(&described.1, "ClusterSchedulerConfigArn"),
        sagemaker_arn(home, "cluster-scheduler-config", &held),
        "case the held policy: ClusterSchedulerConfigArn, got {described:?}"
    );
}

/// The status of a `(status, body)` pair, named so a control reads as one assertion.
fn status_of(answered: &(u16, Value)) -> u16 {
    answered.0
}

/// A `CreateComputeQuota` body with only the four required members (smithy_query
/// shape=CreateComputeQuotaRequest: `Name`, `ClusterArn`, `ComputeQuotaConfig`,
/// `ComputeQuotaTarget`).
fn minimal_quota(name: &str, cluster: &str) -> Value {
    json!({
        "Name": name,
        "ClusterArn": cluster,
        "ComputeQuotaConfig": {},
        "ComputeQuotaTarget": { "TeamName": "research" },
    })
}

/// RULE: every operation that cannot resolve the compute quota a request names refuses it
/// with the modelled `ResourceNotFound` and the message
/// `Compute Quota with arn <arn> could not be found` — the ARN the requested
/// `ComputeQuotaId` resolves to in the Region the request signed for, unquoted and with no
/// trailing period. A version the quota never reached is the same miss.
#[tokio::test]
async fn an_unknown_compute_quota_is_refused_naming_its_resolved_arn() {
    for region in ["us-east-1", "ap-south-1"] {
        let emulator = Emulator::new();
        let cluster = seed_cluster(&emulator, region, "wording-cluster").await;
        let created = emulator
            .call_in(
                region,
                "CreateComputeQuota",
                &minimal_quota("wording-quota", &cluster),
            )
            .await;
        assert_eq!(
            status_of(&created),
            200,
            "the quota fixture must be created, got {created:?}"
        );
        let held = str_field(&created.1, "ComputeQuotaId");

        let mut rows: Vec<(String, Value, String)> = Vec::new();
        for (case, id) in UNMINTED_IDS {
            rows.push((
                format!("{region}/{case}"),
                json!({ "ComputeQuotaId": id }),
                (*id).to_string(),
            ));
        }
        for version in UNREACHED_VERSIONS {
            rows.push((
                format!("{region}/a version the quota never reached: {version}"),
                json!({ "ComputeQuotaId": held, "ComputeQuotaVersion": version }),
                held.clone(),
            ));
        }

        for (case, body, named) in rows {
            let (status, refused) = emulator
                .call_in(region, "DescribeComputeQuota", &body)
                .await;
            let expected = format!(
                "Compute Quota with arn {} could not be found",
                sagemaker_arn(region, "compute-quota", &named)
            );
            assert_refusal(&case, status, &refused, "ResourceNotFound", &expected);
            assert!(
                !message(&refused).ends_with('.'),
                "case {case}: the sentence carries no trailing period, got {:?}",
                message(&refused)
            );
        }

        // Control: the quota the account holds is served.
        let described = emulator
            .call_in(
                region,
                "DescribeComputeQuota",
                &json!({ "ComputeQuotaId": held, "ComputeQuotaVersion": 1 }),
            )
            .await;
        assert_eq!(
            str_field(&described.1, "ComputeQuotaArn"),
            sagemaker_arn(region, "compute-quota", &held),
            "case {region}/the held quota: ComputeQuotaArn, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// JumpStart hubs: addressed by name or by ARN, reported by NAME.
// ---------------------------------------------------------------------------

/// Every implemented operation that resolves a hub, with the body each takes for `hub`.
fn hub_readers(hub: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("DescribeHub", json!({ "HubName": hub })),
        ("DeleteHub", json!({ "HubName": hub })),
        (
            "ListHubContents",
            json!({ "HubName": hub, "HubContentType": "Model" }),
        ),
    ]
}

/// RULE: every operation that cannot resolve the hub a request names refuses it with the
/// modelled `ResourceNotFound` and the message `Hub with name <HubName> does not exist.` —
/// the hub's NAME, which is what both accepted forms of the identifier resolve to, never an
/// ARN.
#[tokio::test]
async fn an_unknown_hub_is_refused_naming_the_hub_by_name() {
    let region = "us-east-1";
    let emulator = Emulator::new();
    // A hub that does exist, so each refusal below is about the identifier rather than an
    // empty store.
    let created = emulator
        .ok(
            "CreateHub",
            &json!({ "HubName": "stored-hub", "HubDescription": "a hub" }),
        )
        .await;
    assert_eq!(
        str_field(&created, "HubArn"),
        sagemaker_arn(region, "hub", "stored-hub"),
        "the hub fixture's ARN, got {created:?}"
    );

    // Rows: the identifier's FORM (plain name, this account's hub ARN, the public-hub
    // `aws` account form the modelled pattern admits, an ARN of another Region) and the
    // name's SHAPE (`HubName` is `len 0..63`, `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`,
    // smithy_query operation=DescribeHub: one character, 63 characters, digit-leading,
    // mixed case), plus a near miss of the stored hub.
    let longest = format!("h{}", "a".repeat(62));
    let names = [
        ("the shortest name", "h".to_string()),
        ("the longest name", longest),
        ("a digit-leading name", "9thhub".to_string()),
        ("a mixed-case name", "Absent-Hub-9".to_string()),
        ("a near miss of the stored hub", "stored-hubs".to_string()),
    ];
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for (case, name) in &names {
        rows.push((format!("{case}, by name"), name.clone(), name.clone()));
        rows.push((
            format!("{case}, by ARN"),
            sagemaker_arn(region, "hub", name),
            name.clone(),
        ));
        rows.push((
            format!("{case}, by an ARN of another Region"),
            sagemaker_arn("eu-west-1", "hub", name),
            name.clone(),
        ));
        rows.push((
            format!("{case}, by a public-hub ARN"),
            format!("arn:aws:sagemaker:{region}:aws:hub/{name}"),
            name.clone(),
        ));
    }

    for (case, identifier, named) in rows {
        for (operation, body) in hub_readers(&identifier) {
            let case = format!("{case}/{operation}");
            let (status, refused) = emulator.call_in(region, operation, &body).await;
            let expected = format!("Hub with name {named} does not exist.");
            assert_refusal(&case, status, &refused, "ResourceNotFound", &expected);
            assert!(
                !message(&refused).contains(':'),
                "case {case}: the sentence names no ARN, got {:?}",
                message(&refused)
            );
        }
    }

    // Control: the hub the account holds is still served, by either form of its name.
    for identifier in [
        "stored-hub".to_string(),
        sagemaker_arn(region, "hub", "stored-hub"),
    ] {
        let described = emulator
            .ok("DescribeHub", &json!({ "HubName": identifier }))
            .await;
        assert_eq!(
            str_field(&described, "HubName"),
            "stored-hub",
            "case {identifier}: the stored hub must be served, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Model cards: addressed by name or by ARN, reported by ARN.
// ---------------------------------------------------------------------------

/// The smallest document that is still model card content.
const CARD_CONTENT: &str = "{}";

/// Every implemented operation that resolves a model card, with the body each takes for
/// `card`. `arn_accepted` is false for the one whose member is an `EntityName` rather than
/// a `ModelCardNameOrArn` (smithy_query operation=DeleteModelCard).
fn model_card_readers(card: &str) -> Vec<(&'static str, Value, bool)> {
    vec![
        ("DescribeModelCard", json!({ "ModelCardName": card }), true),
        (
            "UpdateModelCard",
            json!({ "ModelCardName": card, "ModelCardStatus": "Draft" }),
            true,
        ),
        ("DeleteModelCard", json!({ "ModelCardName": card }), false),
        (
            "CreateModelCardExportJob",
            json!({
                "ModelCardName": card,
                "ModelCardExportJobName": "an-export",
                "OutputConfig": { "S3OutputPath": "s3://a-bucket/cards/" },
            }),
            true,
        ),
    ]
}

/// RULE: every operation that cannot resolve the model card a request names refuses it with
/// the modelled `ResourceNotFound` and the message `Modelcard <arn> does not exist` — the
/// ARN the requested identifier resolves to in the Region the request signed for, unquoted
/// and with no trailing period. A version the card never reached is the same miss.
#[tokio::test]
async fn an_unknown_model_card_is_refused_naming_its_resolved_arn() {
    let region = "eu-central-1";
    let emulator = Emulator::new();
    let created = emulator
        .call_in(
            region,
            "CreateModelCard",
            &json!({
                "ModelCardName": "stored-card",
                "Content": CARD_CONTENT,
                "ModelCardStatus": "Draft",
            }),
        )
        .await;
    assert_eq!(
        status_of(&created),
        200,
        "the model card fixture must be created, got {created:?}"
    );

    // Rows: the name's shape (`EntityName` is `len 1..63`, smithy_query
    // operation=CreateModelCard) and a near miss of the stored card.
    let longest = format!("c{}", "a".repeat(62));
    let names = [
        ("the shortest name", "c".to_string()),
        ("the longest name", longest),
        ("a digit-leading name", "9thcard".to_string()),
        ("a mixed-case name", "Absent-Card-9".to_string()),
        ("a near miss of the stored card", "stored-cards".to_string()),
    ];

    for (case, name) in &names {
        for (operation, body, arn_accepted) in model_card_readers(name) {
            let case = format!("{case}, by name/{operation}");
            let (status, refused) = emulator.call_in(region, operation, &body).await;
            let expected = format!(
                "Modelcard {} does not exist",
                sagemaker_arn(region, "model-card", name)
            );
            assert_refusal(&case, status, &refused, "ResourceNotFound", &expected);
            assert!(
                !message(&refused).ends_with('.'),
                "case {case}: the sentence carries no trailing period, got {:?}",
                message(&refused)
            );

            // The same card addressed by an ARN: the refusal names the ARN the request
            // carried — the one this account and Region resolve to for the Region's own
            // form, and the foreign one as it arrived, since an ARN of another Region or
            // account names no card here and rewriting it would report an identifier the
            // caller never sent.
            if !arn_accepted {
                continue;
            }
            let arn_forms = [
                ("this Region", sagemaker_arn(region, "model-card", name)),
                (
                    "another Region",
                    sagemaker_arn("us-west-2", "model-card", name),
                ),
                (
                    "another account",
                    format!("arn:aws:sagemaker:{region}:999999999999:model-card/{name}"),
                ),
            ];
            for (form, by_arn) in arn_forms {
                let (status, refused) = emulator
                    .call_in(
                        region,
                        operation,
                        &replace_member(&body, "ModelCardName", &by_arn),
                    )
                    .await;
                assert_refusal(
                    &format!("{case}, by an ARN of {form}"),
                    status,
                    &refused,
                    "ResourceNotFound",
                    &format!("Modelcard {by_arn} does not exist"),
                );
            }
        }
    }

    // A version the stored card never reached is as absent as a card nobody created, and
    // the refusal names the card.
    let expected = format!(
        "Modelcard {} does not exist",
        sagemaker_arn(region, "model-card", "stored-card")
    );
    for version in UNREACHED_VERSIONS {
        let (status, refused) = emulator
            .call_in(
                region,
                "DescribeModelCard",
                &json!({ "ModelCardName": "stored-card", "ModelCardVersion": version }),
            )
            .await;
        assert_refusal(
            &format!("a version the card never reached: {version}"),
            status,
            &refused,
            "ResourceNotFound",
            &expected,
        );
    }

    // Control: the card the account holds, at the version it is at, is served.
    let described = emulator
        .call_in(
            region,
            "DescribeModelCard",
            &json!({ "ModelCardName": "stored-card", "ModelCardVersion": 1 }),
        )
        .await;
    assert_eq!(
        str_field(&described.1, "ModelCardArn"),
        sagemaker_arn(region, "model-card", "stored-card"),
        "case the held card: ModelCardArn, got {described:?}"
    );
}

/// `body` with `member` set to `value` — how one row drives the same operation with the
/// other accepted form of its identifier.
fn replace_member(body: &Value, member: &str, value: &str) -> Value {
    let mut replaced = body.clone();
    replaced[member] = json!(value);
    replaced
}

// ---------------------------------------------------------------------------
// Model packages: addressed by name, by `<group>/<version>` or by ARN.
// ---------------------------------------------------------------------------

/// RULE: every operation that cannot resolve the model package a request names refuses it
/// with `ModelPackage <arn> does not exist.` — the package's ARN, resolved from a name or a
/// `<group>/<version>` pair against the Region the request signed for, and reported as it
/// arrived when the request already carried an ARN.
#[tokio::test]
async fn an_unknown_model_package_is_refused_naming_its_arn() {
    let region = "us-east-1";
    let emulator = Emulator::new();
    let created = emulator
        .call_in(
            region,
            "CreateModelPackage",
            &json!({ "ModelPackageName": "stored-package" }),
        )
        .await;
    assert_eq!(
        status_of(&created),
        200,
        "the model package fixture must be created, got {created:?}"
    );

    // Rows: the identifier's form (a bare name, a `<group>/<version>` pair, an ARN of this
    // account and Region, an ARN of another Region, an ARN of another account) with the ARN
    // the refusal must name for each.
    let rows: [(&str, String, String); 5] = [
        (
            "a name nothing was registered under",
            "absent".to_string(),
            sagemaker_arn(region, "model-package", "absent"),
        ),
        (
            "a group and version nothing holds",
            "absent/3".to_string(),
            sagemaker_arn(region, "model-package", "absent/3"),
        ),
        (
            "an ARN of this account",
            sagemaker_arn(region, "model-package", "absent"),
            sagemaker_arn(region, "model-package", "absent"),
        ),
        (
            "an ARN of another Region",
            sagemaker_arn("eu-west-1", "model-package", "absent"),
            sagemaker_arn("eu-west-1", "model-package", "absent"),
        ),
        (
            "an ARN of another account",
            "arn:aws:sagemaker:us-east-1:999999999999:model-package/absent".to_string(),
            "arn:aws:sagemaker:us-east-1:999999999999:model-package/absent".to_string(),
        ),
    ];

    for (case, identifier, named) in rows {
        // `DescribeModelPackage` and `DeleteModelPackage` take a `VersionedArnOrName`;
        // `UpdateModelPackage`'s member is an ARN and nothing else (smithy_query
        // operation=UpdateModelPackage), so it runs only on the ARN rows.
        let mut operations: Vec<(&str, Value)> = vec![
            (
                "DescribeModelPackage",
                json!({ "ModelPackageName": identifier }),
            ),
            (
                "DeleteModelPackage",
                json!({ "ModelPackageName": identifier }),
            ),
        ];
        if identifier.starts_with("arn:") {
            operations.push((
                "UpdateModelPackage",
                json!({ "ModelPackageArn": identifier, "ModelApprovalStatus": "Approved" }),
            ));
        }

        for (operation, body) in operations {
            let case = format!("{case}/{operation}");
            let (status, refused) = emulator.call_in(region, operation, &body).await;
            let expected = format!("ModelPackage {named} does not exist.");
            assert_refusal(&case, status, &refused, "ValidationException", &expected);
        }
    }

    // Control: the package the account holds is still served and was not deleted by any
    // refusal above.
    let described = emulator
        .call_in(
            region,
            "DescribeModelPackage",
            &json!({ "ModelPackageName": "stored-package" }),
        )
        .await;
    assert_eq!(
        str_field(&described.1, "ModelPackageArn"),
        sagemaker_arn(region, "model-package", "stored-package"),
        "case the held package: ModelPackageArn, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// Partner AI apps: a FIXED sentence naming no identifier at all.
// ---------------------------------------------------------------------------

/// RULE: every operation that cannot resolve the partner AI app a request names refuses it
/// with the modelled `ResourceNotFound` and the fixed message `App entity does not exist.`,
/// which carries no identifier — so the ARN the caller probed with is never echoed back.
#[tokio::test]
async fn an_unknown_partner_app_is_refused_with_the_fixed_sentence() {
    let emulator = Emulator::new();

    // Rows: well-formed `PartnerAppArn`s (`…:partner-app/app-[A-Z0-9]{12}`, smithy_query
    // operation=DescribePartnerApp) differing in the app id, the Region and the account.
    let rows = [
        (
            "an app id nothing minted",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:partner-app/app-ABCDEFGH1234"),
        ),
        (
            "another app id",
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:partner-app/app-000000000000"),
        ),
        (
            "another Region",
            format!("arn:aws:sagemaker:eu-west-1:{ACCOUNT}:partner-app/app-ABCDEFGH1234"),
        ),
        (
            "another account",
            "arn:aws:sagemaker:us-east-1:999999999999:partner-app/app-ABCDEFGH1234".to_string(),
        ),
    ];

    for (case, arn) in rows {
        for operation in ["DescribePartnerApp", "CreatePartnerAppPresignedUrl"] {
            let case = format!("{case}/{operation}");
            let (status, refused) = emulator.call(operation, &json!({ "Arn": arn })).await;
            assert_refusal(
                &case,
                status,
                &refused,
                "ResourceNotFound",
                "App entity does not exist.",
            );
            assert!(
                !message(&refused).contains(&arn),
                "case {case}: the refusal must not echo the probed ARN, got {:?}",
                message(&refused)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Inference Recommender jobs: addressed by name, reported by name.
// ---------------------------------------------------------------------------

/// RULE: an operation that cannot resolve the Inference Recommender job a request names
/// refuses it with the modelled `ResourceNotFound`, naming the job by the name the request
/// carried, unquoted — the job's own reader qualifying it by the account it looked in
/// (`Inference recommendations job <JobName> for <account> not found.`) and a surface that
/// reports OVER a job stating it as `Inference recommendations job <JobName> was not found.`
#[tokio::test]
async fn an_unknown_recommendations_job_is_refused_naming_the_requested_name() {
    let emulator = Emulator::new();

    // Rows: the name's shape (`RecommendationJobName` is `len 1..64`,
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,63}$`, smithy_query
    // operation=DescribeInferenceRecommendationsJob).
    let longest = format!("j{}", "a".repeat(63));
    let rows = [
        ("the shortest name", "j".to_string()),
        ("the longest name", longest),
        ("a digit-leading name", "9thjob".to_string()),
        ("a mixed-case name", "Absent-Job-9".to_string()),
        ("a hyphenated name", "absent-recommendations-job".to_string()),
    ];

    for (case, name) in rows {
        let operations: [(&str, Value, String); 2] = [
            (
                "DescribeInferenceRecommendationsJob",
                json!({ "JobName": name }),
                format!("Inference recommendations job {name} for {ACCOUNT} not found."),
            ),
            (
                "GetScalingConfigurationRecommendation",
                json!({
                    "InferenceRecommendationsJobName": name,
                    "EndpointName": "a-benchmarked-endpoint",
                }),
                format!("Inference recommendations job {name} was not found."),
            ),
        ];
        for (operation, body, expected) in operations {
            let case = format!("{case}/{operation}");
            let (status, refused) = emulator.call(operation, &body).await;
            assert_refusal(&case, status, &refused, "ResourceNotFound", &expected);
        }
    }
}

// ---------------------------------------------------------------------------
// HyperPod clusters addressed by `ClusterName`.
// ---------------------------------------------------------------------------

/// Every implemented operation that resolves a `ClusterNameOrArn`, with the body each takes
/// for `cluster`. The node-batch operations carry the smallest well-formed work item they
/// model, so the cluster is the only thing the request can miss on.
fn cluster_readers(cluster: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("DescribeCluster", json!({ "ClusterName": cluster })),
        ("DeleteCluster", json!({ "ClusterName": cluster })),
        ("ListClusterNodes", json!({ "ClusterName": cluster })),
        (
            "BatchAddClusterNodes",
            json!({
                "ClusterName": cluster,
                "NodesToAdd": [{ "InstanceGroupName": "workers", "IncrementTargetCountBy": 1 }],
            }),
        ),
        (
            "BatchReplaceClusterNodes",
            json!({ "ClusterName": cluster, "NodeIds": ["i-0abcdef1"] }),
        ),
    ]
}

/// RULE: every operation that cannot resolve the HyperPod cluster its `ClusterName` names
/// refuses it with the modelled `ResourceNotFound` and the message
/// `Cluster with name <ClusterName> not found` — the identifier the request carried,
/// unquoted and with no trailing period.
#[tokio::test]
async fn an_unknown_cluster_is_refused_naming_the_requested_identifier() {
    let region = "us-east-1";
    let emulator = Emulator::new();
    // A cluster that does exist, so each refusal below is about the identifier.
    let held = seed_cluster(&emulator, region, "stored-cluster").await;

    // Rows: the name's shape (`ClusterName` is `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`,
    // smithy_query operation=DescribeCluster), a near miss of the stored cluster, and a
    // well-formed `ClusterArn` whose minted id nothing holds — a cluster's ARN carries that
    // id rather than its name, so the refusal reports the identifier as the request sent it.
    let longest = format!("c{}", "a".repeat(62));
    let rows = [
        ("the shortest name", "c".to_string()),
        ("the longest name", longest),
        ("a digit-leading name", "9thcluster".to_string()),
        ("a mixed-case name", "Absent-Cluster-9".to_string()),
        (
            "a near miss of the stored cluster",
            "stored-clusters".to_string(),
        ),
        (
            // This row pins the crate's documented choice for a form the recorded units do
            // not cover (they send a `ClusterName`): the identifier is ECHOED, because a
            // cluster's ARN carries the minted cluster id rather than its name, so nothing
            // could be resolved for a cluster that does not exist.
            "an ARN whose id nothing holds",
            sagemaker_arn(region, "cluster", "0123456789ab"),
        ),
    ];

    for (case, identifier) in rows {
        for (operation, body) in cluster_readers(&identifier) {
            let case = format!("{case}/{operation}");
            let (status, refused) = emulator.call_in(region, operation, &body).await;
            let expected = format!("Cluster with name {identifier} not found");
            assert_refusal(&case, status, &refused, "ResourceNotFound", &expected);
            assert!(
                !message(&refused).ends_with('.'),
                "case {case}: the sentence carries no trailing period, got {:?}",
                message(&refused)
            );
        }
    }

    // Control: the cluster the account holds is served, by either form of its identifier,
    // and no refusal above deleted it.
    for identifier in ["stored-cluster".to_string(), held.clone()] {
        let described = emulator
            .call_in(
                region,
                "DescribeCluster",
                &json!({ "ClusterName": identifier }),
            )
            .await;
        assert_eq!(
            str_field(&described.1, "ClusterArn"),
            held,
            "case {identifier}: the stored cluster must be served, got {described:?}"
        );
    }
}
