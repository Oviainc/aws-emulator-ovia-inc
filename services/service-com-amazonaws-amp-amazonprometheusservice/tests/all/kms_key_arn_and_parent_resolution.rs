// SPDX-License-Identifier: CC0-1.0

//! Two rules of the aps request boundary, each driven end to end over HTTP and each
//! asserted over a TABLE spanning what it generalises over rather than over one call:
//!
//! 1. **An unresolvable PARENT is a not-found, not a constraint failure.** A request a
//!    workspace-scoped operation cannot place in a workspace this account holds — the
//!    `/workspaces/{workspaceId}/...` `@httpLabel` resolving to nothing — is answered HTTP
//!    404 `ResourceNotFoundException` carrying the unresolved id in `resourceId` and
//!    `AWS::APS::Workspace` in `resourceType`, whichever operation looked it up and
//!    whether that operation reads the parent or creates a child inside it. A member the
//!    caller got wrong is still reported as the 400 first.
//! 2. **`KmsKeyArn`'s key-id segment is unconstrained.** A `kmsKeyArn` is accepted when
//!    it is a KMS key ARN — partition, `kms`, region, a 12-digit account and a `key/`
//!    segment — whatever the key id after `key/` looks like, and the requirement a
//!    refusal reports quotes that same constraint. See the "KmsKeyArn's key-id segment"
//!    section on `crate::ops::create_workspace` and `crate::state::check_kms_key_arn` for
//!    where the constraint comes from.
//!
//! The harness (`Amp`, `Reply`, `text`) is the one `tests/all/workspace_lifecycle.rs`
//! owns: one `MultiServiceRouter` with this service mounted on one shared engine, so
//! routing, the `restJson1` wire shapes and the modelled error envelopes are all
//! exercised.

use serde_json::{json, Value};

use crate::workspace_lifecycle::{text, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    VALIDATION_REASON, WORKSPACE_NOT_FOUND_MESSAGE, WORKSPACE_RESOURCE_TYPE,
};

/// `WorkspaceId`s no store mints (`[0-9A-Za-z][-.0-9A-Z_a-z]*`, length 1..64), in three
/// different FORMS so the rule is not read off one id shape: the `ws-`-plus-UUID form the
/// service mints, the `ws-` plus bare 26 characters a recorded client sent, and a bare
/// word carrying no `ws-` prefix at all.
const ABSENT_WORKSPACES: [&str; 3] = [
    "ws-00000000-0000-0000-0000-000000000000",
    "ws-00000000000000000000000000",
    "no.such_workspace-1",
];

/// The requirement a `kmsKeyArn` outside the constraint this service applies reports, as
/// `state::constraint_requirement` renders a `@pattern` (`state::KMS_KEY_ARN_PATTERN`,
/// with the `^`/`$` anchors dropped).
const KMS_KEY_ARN_REQUIREMENT: &str = "Member must satisfy regular expression pattern: \
     arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+";

/// A `randomCutForest` configuration whose query is wrapped by a supported PromQL
/// aggregation operator, so `CreateAnomalyDetector` gets past its own configuration check
/// and reaches the parent-resolution branch under test.
fn detector_configuration() -> Value {
    json!({ "randomCutForest": { "query": "sum(up)" } })
}

/// A CloudWatch Logs log group ARN satisfying `CreateLoggingConfiguration$logGroupArn`'s
/// pattern `^arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:[A-Za-z0-9\.\-\_\#/]{1,512}\:\*$`.
fn log_group_arn(region: &str) -> String {
    format!(
        "arn:aws:logs:{region}:{}:log-group:/aws/vendedlogs/amp:*",
        engine::emulator_account_id()
    )
}

/// The `destinations` member of a query logging configuration: `LoggingDestinations` is
/// modelled `len 1..1`, so exactly one.
fn destinations_json(region: &str) -> Value {
    json!([{
        "cloudWatchLogs": { "logGroupArn": log_group_arn(region) },
        "filters": { "qspThreshold": 10 },
    }])
}

/// A workspace-scoped request: the operation it names, its HTTP binding and its body.
struct Scoped {
    operation: &'static str,
    method: &'static str,
    /// The path under `/workspaces/{workspaceId}`, empty for the workspace itself.
    suffix: &'static str,
    body: Value,
}

/// The requests the table drives against a workspace that does not resolve — four
/// operations that CREATE a child inside the parent, one that attaches a document to it
/// and one that merely reads the parent, over five HTTP methods. The rule is a property of
/// the path label, so a reporting operation and a mutating one must answer it alike.
fn workspace_scoped_requests(region: &str) -> Vec<Scoped> {
    vec![
        Scoped {
            operation: "CreateAnomalyDetector",
            method: "POST",
            suffix: "/anomalydetectors",
            body: json!({
                "alias": "orphan",
                "configuration": detector_configuration(),
                "evaluationIntervalInSeconds": 60,
            }),
        },
        Scoped {
            operation: "CreateRuleGroupsNamespace",
            method: "POST",
            suffix: "/rulegroupsnamespaces",
            body: json!({ "name": "rules", "data": "Z3JvdXBzOiBbXQ==" }),
        },
        Scoped {
            operation: "CreateLoggingConfiguration",
            method: "POST",
            suffix: "/logging",
            body: json!({ "logGroupArn": log_group_arn(region) }),
        },
        Scoped {
            operation: "CreateQueryLoggingConfiguration",
            method: "POST",
            suffix: "/logging/query",
            body: json!({ "destinations": destinations_json(region) }),
        },
        Scoped {
            operation: "PutResourcePolicy",
            method: "PUT",
            suffix: "/policy",
            body: json!({ "policyDocument": "{}" }),
        },
        Scoped {
            operation: "DescribeWorkspace",
            method: "GET",
            suffix: "",
            body: Value::Null,
        },
    ]
}

/// Rule 1 over the whole class: every workspace-scoped operation answers an unresolvable
/// `{workspaceId}` with the modelled 404, the constant not-found sentence, the id in
/// `resourceId` and the parent's AWS type in `resourceType` — never a 400 naming the
/// member, which is what a client that branches on the status class and the error code
/// would read as a malformed request rather than an absent parent.
#[tokio::test]
async fn an_unresolvable_parent_workspace_is_a_not_found() {
    let region = "us-west-2";
    let amp = Amp::new();

    for absent in ABSENT_WORKSPACES {
        for request in workspace_scoped_requests(region) {
            let case = format!("{} on {absent}", request.operation);
            let body = match &request.body {
                Value::Null => String::new(),
                body => body.to_string(),
            };
            let reply = amp
                .call(
                    request.method,
                    &format!("/workspaces/{absent}{}", request.suffix),
                    region,
                    &body,
                )
                .await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: an absent parent workspace must answer 404, got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected the modelled ResourceNotFoundException, got {reply:?}"
            );
            assert_eq!(
                reply.header("x-amzn-errortype").unwrap_or_default(),
                "ResourceNotFoundException",
                "case {case:?}: the error code a client reads off the header must match, \
                 got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["message"]),
                WORKSPACE_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the constant not-found sentence, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["resourceId"]),
                absent,
                "case {case:?}: the unresolved id belongs in `resourceId`, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["resourceType"]),
                WORKSPACE_RESOURCE_TYPE,
                "case {case:?}: `resourceType` must name the parent's AWS type, got {reply:?}"
            );
            assert!(
                reply.body["fieldList"].is_null(),
                "case {case:?}: an absent parent is not a member-validation failure, \
                 got {reply:?}"
            );
        }
    }
}

/// The other half of rule 1: the 404 branch refuses only what does not resolve. A detector
/// created inside a workspace this account DOES hold is minted and reads back through
/// `DescribeAnomalyDetector`, and the same workspace answers the unresolvable-parent 404
/// for a sibling id.
#[tokio::test]
async fn a_resolvable_parent_still_takes_a_child() {
    let region = "eu-west-1";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "parent").await;

    let created = amp
        .call(
            "POST",
            &format!("/workspaces/{workspace_id}/anomalydetectors"),
            region,
            &json!({ "alias": "child", "configuration": detector_configuration() }).to_string(),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "case \"resolvable parent\": CreateAnomalyDetector must mint the detector, \
         got {created:?}"
    );
    let detector_id = text(&created.body["anomalyDetectorId"]);
    assert!(
        !detector_id.is_empty(),
        "case \"resolvable parent\": the response must carry the minted id, got {created:?}"
    );

    let described = amp
        .call(
            "GET",
            &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
            region,
            "",
        )
        .await;
    assert_eq!(
        described.status, 200,
        "case \"round trip\": the minted detector must read back, got {described:?}"
    );
    assert_eq!(
        text(&described.body["anomalyDetector"]["anomalyDetectorId"]),
        detector_id,
        "case \"round trip\": the stored detector must report the minted id, got {described:?}"
    );
    assert_eq!(
        text(&described.body["anomalyDetector"]["alias"]),
        "child",
        "case \"round trip\": the stored detector must report the alias it was created \
         with, got {described:?}"
    );

    // A workspace that exists does not make a SIBLING id resolve.
    let orphan = amp
        .call(
            "POST",
            &format!("/workspaces/{}/anomalydetectors", ABSENT_WORKSPACES[0]),
            region,
            &json!({ "alias": "orphan", "configuration": detector_configuration() }).to_string(),
        )
        .await;
    assert_eq!(
        orphan.status, 404,
        "case \"sibling id\": an absent workspace must still answer 404, got {orphan:?}"
    );
}

/// Rule 2, the accepting half: every key-ARN form AWS admits is accepted and round-trips
/// through `DescribeWorkspace`. The rows differ along what the constraint generalises
/// over — the key-id's character set (lowercase hex, uppercase hex, a `mrk-` multi-region
/// prefix, a single character, a key id carrying `/`), the partition (`aws`, `aws-cn`,
/// `aws-us-gov`) and the region — and the first row is the only one the issue cited.
#[tokio::test]
async fn every_kms_key_id_form_is_accepted_and_stored() {
    let account = engine::emulator_account_id();
    // (case, the kmsKeyArn submitted)
    let cases: Vec<(&str, String)> = vec![
        (
            "a plain UUID key id",
            format!("arn:aws:kms:us-west-2:{account}:key/1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d"),
        ),
        (
            "a multi-region key id",
            format!("arn:aws:kms:us-west-2:{account}:key/mrk-0123456789abcdef0123456789abcdef"),
        ),
        (
            "an uppercase-hex key id",
            format!("arn:aws:kms:us-east-1:{account}:key/1A2B3C4D-5E6F-7A8B-9C0D-1E2F3A4B5C6D"),
        ),
        (
            "a one-character key id",
            format!("arn:aws:kms:ap-south-1:{account}:key/k"),
        ),
        (
            "a key id carrying a slash",
            format!("arn:aws:kms:eu-west-1:{account}:key/tenant/metrics.key_1"),
        ),
        (
            "the China partition",
            format!("arn:aws-cn:kms:cn-north-1:{account}:key/mrk-fedcba98765432100123456789abcdef"),
        ),
        (
            "the GovCloud partition",
            format!(
                "arn:aws-us-gov:kms:us-gov-west-1:{account}:key/\
                 4e2f3a4b-5c6d-7e8f-9a0b-1c2d3e4f5a6b"
            ),
        ),
    ];

    let region = "us-west-2";
    let amp = Amp::new();
    for (case, kms_key_arn) in cases {
        let created = amp
            .call(
                "POST",
                "/workspaces",
                region,
                &json!({ "alias": "encrypted", "kmsKeyArn": kms_key_arn }).to_string(),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: a KMS key ARN the service accepts must be accepted, got {created:?}"
        );
        assert_eq!(
            text(&created.body["kmsKeyArn"]),
            kms_key_arn,
            "case {case:?}: the response must report the key the workspace holds, got {created:?}"
        );

        let workspace_id = text(&created.body["workspaceId"]);
        let described = amp
            .call("GET", &format!("/workspaces/{workspace_id}"), region, "")
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the new workspace must read back, got {described:?}"
        );
        assert_eq!(
            text(&described.body["workspace"]["kmsKeyArn"]),
            kms_key_arn,
            "case {case:?}: the stored workspace must report the key it was created with, \
             got {described:?}"
        );
    }
}

/// Rule 2, the refusing half: a `kmsKeyArn` that is not a KMS key ARN is still refused —
/// one row per segment of the constraint (the partition prefix, the service, the region's
/// character set, the 12-digit account, the `key/` segment and its non-empty id) — and the
/// requirement reported names the constraint the service applies, with the submitted value
/// nowhere in the body. Every value is at least 20 characters, the member's modelled
/// minimum length, so the pattern is the only constraint at fault.
#[tokio::test]
async fn a_kms_key_arn_outside_the_pattern_is_refused() {
    let account = engine::emulator_account_id();
    let cases: Vec<(&str, String)> = vec![
        (
            "no ARN at all",
            "not-a-valid-kms-arn-at-all".to_string(),
        ),
        (
            "an alias segment instead of key/",
            format!("arn:aws:kms:us-west-2:{account}:alias/my-workspace-key"),
        ),
        (
            "an empty key id",
            format!("arn:aws:kms:us-west-2:{account}:key/"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:s3:us-west-2:{account}:key/1a2b3c4d5e6f7a8b"),
        ),
        (
            "an account id one digit short",
            "arn:aws:kms:us-west-2:12345678901:key/1a2b3c4d5e6f7a8b".to_string(),
        ),
        (
            "an uppercase region",
            format!("arn:aws:kms:US-WEST-2:{account}:key/1a2b3c4d5e6f7a8b"),
        ),
    ];

    let region = "us-west-2";
    let amp = Amp::new();
    for (case, kms_key_arn) in cases {
        let reply = amp
            .call(
                "POST",
                "/workspaces",
                region,
                &json!({ "alias": "refused", "kmsKeyArn": kms_key_arn }).to_string(),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a value outside the pattern must be refused 400, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["reason"]),
            VALIDATION_REASON,
            "case {case:?}: `reason` is @required, got {reply:?}"
        );
        assert_eq!(
            reply.body["fieldList"],
            json!([{ "name": "kmsKeyArn", "message": KMS_KEY_ARN_REQUIREMENT }]),
            "case {case:?}: expected one fieldList entry naming the member and the \
             constraint the service applies, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            format!("Invalid kmsKeyArn: {KMS_KEY_ARN_REQUIREMENT}"),
            "case {case:?}: expected the message to name the member and its requirement, \
             got {reply:?}"
        );
        assert!(
            !reply.body.to_string().contains(&kms_key_arn),
            "case {case:?}: the refusal must not echo the submitted value {kms_key_arn:?}, \
             got {reply:?}"
        );
    }
}

/// The member's MODELLED length bound (20..2048) is still enforced — it is re-declared
/// beside the corrected pattern rather than dropped with it, so taking the pattern check
/// off the request boundary did not take the length check with it. Each published bound is
/// exercised at the bound and one past it.
#[tokio::test]
async fn the_members_modelled_length_bound_is_still_enforced() {
    let region = "us-west-2";
    let amp = Amp::new();
    // A well-formed key ARN prefix, so padding the key id is what moves the LENGTH.
    let prefix = format!(
        "arn:aws:kms:us-west-2:{}:key/",
        engine::emulator_account_id()
    );
    // `KmsKeyArn` is modelled `@length(min: 20, max: 2048)`
    // (`smithy_query(query="shape", shape="KmsKeyArn")`).
    let (min, max) = (20usize, 2048usize);
    let padded = |total: usize| format!("{prefix}{}", "a".repeat(total - prefix.len()));

    // (case, the value, the expected status, the requirement reported when refused)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "at the published maximum length",
            padded(max),
            202,
            "",
        ),
        (
            "one past the published maximum length",
            padded(max + 1),
            400,
            "Member must have length less than or equal to 2048",
        ),
        (
            // Nothing shorter than the prefix can satisfy the pattern either, so this row
            // asserts only that the refusal names the member once.
            "one below the published minimum length",
            "a".repeat(min - 1),
            400,
            "",
        ),
    ];

    for (case, kms_key_arn, status, requirement) in cases {
        let reply = amp
            .call(
                "POST",
                "/workspaces",
                region,
                &json!({ "alias": "lengths", "kmsKeyArn": kms_key_arn }).to_string(),
            )
            .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected status {status}, got {reply:?}"
        );
        if status < 400 {
            assert_eq!(
                text(&reply.body["kmsKeyArn"]),
                kms_key_arn,
                "case {case:?}: an accepted key must be reported back, got {reply:?}"
            );
            continue;
        }
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {reply:?}"
        );
        let fields = reply.body["fieldList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            fields.len(),
            1,
            "case {case:?}: a refusal names the one member at fault, got {reply:?}"
        );
        assert_eq!(
            text(&fields[0]["name"]),
            "kmsKeyArn",
            "case {case:?}: the fieldList entry must name the member, got {reply:?}"
        );
        if !requirement.is_empty() {
            assert_eq!(
                text(&fields[0]["message"]),
                requirement,
                "case {case:?}: expected the length requirement, got {reply:?}"
            );
        }
    }
}

/// The two kinds of refusal stay distinct, and in the order the service applies them: a
/// member the caller got wrong is reported whatever the parent — a constraint the MODEL
/// declares at the request boundary, and a rule this SERVICE enforces on a member
/// (`RandomCutForestConfiguration$query` must be wrapped by a supported PromQL
/// aggregation operator) in the handler — while a request that is otherwise well formed
/// is the parent's 404.
#[tokio::test]
async fn a_member_the_caller_got_wrong_is_reported_whatever_the_parent() {
    let region = "us-east-2";
    let amp = Amp::new();
    let absent = ABSENT_WORKSPACES[1];
    let uri = format!("/workspaces/{absent}/anomalydetectors");

    // (case, the body, the refusal expected for an absent parent)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a query no supported aggregation operator wraps",
            json!({
                "alias": "unwrapped",
                "configuration": { "randomCutForest": { "query": "up" } },
            }),
            400,
            "ValidationException",
        ),
        (
            "an alias outside the pattern the model declares",
            json!({ "alias": "has spaces", "configuration": detector_configuration() }),
            400,
            "ValidationException",
        ),
        (
            "a well-formed request",
            json!({ "alias": "well-formed", "configuration": detector_configuration() }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, body, status, code) in cases {
        let reply: Reply = amp.call("POST", &uri, region, &body.to_string()).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected status {status}, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case:?}: expected error code {code:?}, got {reply:?}"
        );
    }

    // The same member rule against a parent that DOES resolve is the same refusal, so the
    // 404 branch is about the parent and nothing else.
    let workspace_id = amp.active_workspace(region, "rules").await;
    let refused = amp
        .call(
            "POST",
            &format!("/workspaces/{workspace_id}/anomalydetectors"),
            region,
            &json!({
                "alias": "unwrapped",
                "configuration": { "randomCutForest": { "query": "up" } },
            })
            .to_string(),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"resolvable parent, unwrapped query\": expected the modelled refusal, \
         got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case \"resolvable parent, unwrapped query\": unexpected error code, got {refused:?}"
    );
}
