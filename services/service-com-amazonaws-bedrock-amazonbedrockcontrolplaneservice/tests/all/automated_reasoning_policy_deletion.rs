// SPDX-License-Identifier: CC0-1.0

//! `DeleteAutomatedReasoningPolicy` end-to-end: the revision an
//! `AutomatedReasoningPolicyArn` names is removed from the store, over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A deleted policy is gone from every read**: the delete answers `202` with no
//!    members, a later `GetAutomatedReasoningPolicy` is `404`,
//!    `ListAutomatedReasoningPolicies` reports exactly the policies that are left, and the
//!    name and the `clientRequestToken` the deleted policy held are free again — over
//!    policies carrying each form of the create's optional members, and over the three
//!    spellings of `force` (omitted, so the modelled `@default(false)` applies, `false` and
//!    `true`), which decides nothing for a policy holding no artifact.
//! 2. **A versioned ARN removes only that version**: the `:<version>` suffix
//!    `AutomatedReasoningPolicyArn` admits selects ONE snapshot, so the draft and every
//!    other version read back exactly as they did — for the only version of a policy and
//!    for the first, a middle and the last of several — and a number this policy has cut is
//!    never minted again.
//! 3. **An unforced delete is refused while the policy still holds an artifact**: `force`
//!    "specifies whether to force delete the automated reasoning policy even if it has
//!    active resources. When false, Amazon Bedrock validates if all artifacts have been
//!    deleted (e.g. policy version, test case, test result) for a policy before deletion",
//!    so a delete that did not force it is `ValidationException` for each kind of artifact
//!    the policy can hold — in the one sentence Amazon Bedrock words that validation with,
//!    naming the kind in the way — leaves everything readable, and the forced delete takes
//!    the policy and that artifact with it.
//! 4. **An ARN naming no revision is refused**: `404 ResourceNotFoundException` over
//!    well-formed ARNs whose policy id, whose region or whose version number names nothing,
//!    with `force` set either way — forcing a delete does not conjure the policy.
//! 5. **A policy a guardrail checks against is in use by it**: the case
//!    `ResourceInUseException` itself documents, over the policy's unversioned and versioned
//!    ARNs and over a reference held by the guardrail's draft and by a published version of
//!    it; the policy becomes deletable once nothing references it.
//!
//! Every expectation is derived from the model, cited per rule
//! (`smithy_query(query="operation",operation="DeleteAutomatedReasoningPolicy")`:
//! `@http(code: 202, method: "DELETE", uri: "/automated-reasoning-policies/{policyArn}")`,
//! `force` bound to the `force` query parameter with `@default(false)`,
//! `ValidationException` 400, `ResourceInUseException` 400, `ResourceNotFoundException`
//! 404), and from its
//! documentation. The requests go through `crate::support` — the ONE harness every suite in
//! this crate drives the emulator with.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::automated_reasoning_policy_test_cases::{create_test_case, get_test_case, CHECK_RESULTS};
use crate::support::{emulator, label, seed_build_workflow, Emulator, Reply, ACCOUNT, REGION};

/// The status `DeleteAutomatedReasoningPolicy` answers a delete it accepted with
/// (`@http(code: 202)`).
const DELETE_ACCEPTED: u16 = 202;

/// The `ResourceNotFoundException` message a write of a policy this account holds no record
/// of is refused with — Amazon Bedrock's wording for that error class as the calls that
/// write the policy itself word it, carrying neither the ARN the request sent nor the
/// account it looked in.
const POLICY_NOT_FOUND: &str = "Policy was not found";

/// The same error class as a READ of the policy words it, which is what a
/// `GetAutomatedReasoningPolicy` of a deleted policy must answer
/// (`refusal_message_fidelity.rs` pins it over every operation that reports it).
const POLICY_NOT_FOUND_ON_READ: &str = "Policy does not exist";

/// The `ResourceNotFoundException` message a request naming a version the addressed policy
/// has not cut is refused with: the policy is there and the revision is not.
const POLICY_VERSION_NOT_FOUND: &str = "Policy version does not exist";

/// The `buildWorkflowType` a seeded build workflow carries
/// (`AutomatedReasoningPolicyBuildWorkflowType$INGEST_CONTENT`).
const INGEST_CONTENT: &str = "INGEST_CONTENT";

/// How far in the past a seeded build workflow was started, so the clock has already owed it
/// its completion — the whole progression a build runs through, and a little more — by the
/// time a test workflow is run against it: the one state a test workflow may run against,
/// reached without sleeping for it.
const LONG_BUILT: Duration =
    bedrock::state::BUILD_WORKFLOW_COMPLETE_AFTER.saturating_add(Duration::from_millis(150));

/// How `force` is spelled on the wire.
#[derive(Debug, Clone, Copy)]
enum Force {
    /// Not sent at all, so the member's modelled `@default(false)` applies.
    Omitted,
    /// `force=false`, the default spelled out.
    False,
    /// `force=true`.
    True,
}

impl Force {
    fn query(self) -> &'static str {
        match self {
            Force::Omitted => "",
            Force::False => "?force=false",
            Force::True => "?force=true",
        }
    }
}

/// One `DELETE /automated-reasoning-policies/{policyArn}`; the ARN is a path LABEL and
/// carries `:` and `/`, which are percent-encoded so it stays one path segment, exactly as
/// an SDK writes it.
async fn delete_policy(emu: &Emulator, policy_arn: &str, force: Force) -> Reply {
    emu.call(
        "DELETE",
        &format!(
            "/automated-reasoning-policies/{}{}",
            label(policy_arn),
            force.query()
        ),
        None,
    )
    .await
}

/// `CreateAutomatedReasoningPolicy` with `body` as the request.
async fn create_policy(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/automated-reasoning-policies", Some(body))
        .await
}

/// `GetAutomatedReasoningPolicy` for one revision, by the ARN that names it.
async fn get_policy(emu: &Emulator, policy_arn: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/automated-reasoning-policies/{}", label(policy_arn)),
        None,
    )
    .await
}

/// `CreateAutomatedReasoningPolicyVersion`, held against the draft's current
/// `definitionHash` — the concurrency token the operation requires. Cutting a version leaves
/// the draft alone, so one hash cuts every version of one policy.
async fn create_version(emu: &Emulator, policy_arn: &str, definition_hash: &str) -> Reply {
    emu.call(
        "POST",
        &format!(
            "/automated-reasoning-policies/{}/versions",
            label(policy_arn)
        ),
        Some(json!({ "lastUpdatedDefinitionHash": definition_hash })),
    )
    .await
}

/// `ListAutomatedReasoningPolicies`, which reports the draft of every policy of the region.
async fn list_policies(emu: &Emulator) -> Reply {
    emu.call(
        "GET",
        "/automated-reasoning-policies?maxResults=1000",
        None,
    )
    .await
}

/// Every `policyArn` a listing reports, in the order it reports them.
async fn listed_arns(case: &str, emu: &Emulator) -> Vec<String> {
    let listed = list_policies(emu).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: the listing must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    listed
        .get("automatedReasoningPolicySummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the listing's summaries are a required list, got {:?}",
                listed.body
            )
        })
        .iter()
        .map(|summary| {
            summary
                .get("policyArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Create a policy from `body` and hand back its `(policyArn, definitionHash)` — the hash a
/// later cut of a version is held against.
async fn policy(emu: &Emulator, case: &str, body: Value) -> (String, String) {
    let created = create_policy(emu, body).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the create must answer 200, got {} {:?}",
        created.status, created.body
    );
    (created.text("policyArn"), created.text("definitionHash"))
}

/// Assert `reply` is the answer the delete gives a request it accepted: the modelled status
/// and a body carrying no members (`DeleteAutomatedReasoningPolicyResponse` declares none).
fn assert_accepted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, DELETE_ACCEPTED,
        "case {case:?}: the delete must answer {DELETE_ACCEPTED}, got {} {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.body == Value::Null || reply.body == json!({}),
        "case {case:?}: the delete's answer carries no members, got {:?}",
        reply.body
    );
    assert!(
        reply.error.is_none(),
        "case {case:?}: an accepted delete carries no error, got {:?}",
        reply.error
    );
}

/// Assert `reply` is a refusal carrying `status`, `error` and exactly `message`.
fn assert_refused(case: &str, reply: &Reply, status: u16, error: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (status, Some(error)),
        "case {case:?}: expected {status} {error}, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.get("message"),
        json!(message),
        "case {case:?}: expected the message {message:?}, got {:?}",
        reply.body
    );
}

/// The `message` of a refusal, as the text a row inspects for the artifact kind it named.
fn message_of(case: &str, reply: &Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {:?}", reply.body))
        .to_string()
}

// ---------------------------------------------------------------------------------------
// 1. A deleted policy is gone from every read.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_policy_is_gone_from_every_read() {
    // Rows differ along which optional members the stored policy carried and along how
    // `force` was spelled — a policy holding no artifact is deleted whichever way it is
    // sent, so the member decides nothing here.
    struct Row {
        case: &'static str,
        extra: fn() -> Value,
        force: Force,
    }

    let rows = [
        Row {
            case: "only the required name",
            extra: || json!({}),
            force: Force::Omitted,
        },
        Row {
            case: "a description",
            extra: || json!({ "description": "a policy with prose" }),
            force: Force::False,
        },
        Row {
            case: "a customer managed key and tags",
            extra: || {
                json!({
                    "kmsKeyId": "11111111-2222-4333-8444-555555555555",
                    "tags": [{ "key": "owner", "value": "deletion suite" }],
                })
            },
            force: Force::True,
        },
    ];

    let emu = emulator();
    // A policy nothing in this suite deletes, so each row's listing assertion is against a
    // store that still holds something.
    let (survivor, _) = policy(&emu, "the survivor", json!({ "name": "the survivor" })).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // `AutomatedReasoningPolicyName` is `^[0-9a-zA-Z-_ ]+$`, so the row's index names
        // its policy rather than the case text.
        let name = format!("deleted policy {index}");
        let mut body = (row.extra)();
        body["name"] = json!(name);
        body["clientRequestToken"] = json!(format!("deletion-token-{index}"));
        let (policy_arn, _) = policy(&emu, case, body.clone()).await;

        let before = get_policy(&emu, &policy_arn).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the policy must read back before it is deleted, got {} {:?}",
            before.status, before.body
        );

        assert_accepted(case, &delete_policy(&emu, &policy_arn, row.force).await);

        let read = get_policy(&emu, &policy_arn).await;
        assert_eq!(
            (read.status, read.error.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the deleted policy must be gone from the read, got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            listed_arns(case, &emu).await,
            vec![survivor.clone()],
            "case {case:?}: the listing must report exactly the policies that are left"
        );

        // A second delete has no policy to remove, and Amazon Bedrock reports that rather
        // than accepting it.
        assert_refused(
            &format!("{case}, deleted twice"),
            &delete_policy(&emu, &policy_arn, row.force).await,
            404,
            "ResourceNotFoundException",
            POLICY_NOT_FOUND,
        );

        // The name is free again, and the `clientRequestToken` that minted the policy no
        // longer names it: the create that replays both mints a NEW policy.
        let replay = create_policy(&emu, body).await;
        assert_eq!(
            replay.status, 200,
            "case {case:?}: a create replaying the name and token of a deleted policy must \
             answer 200, got {} {:?}",
            replay.status, replay.body
        );
        let reminted = replay.text("policyArn");
        assert_ne!(
            reminted, policy_arn,
            "case {case:?}: the replay must mint a new policy, not name the deleted one"
        );

        // Leave the store holding only the survivor for the next row.
        assert_accepted(
            &format!("{case}, the re-minted policy"),
            &delete_policy(&emu, &reminted, Force::True).await,
        );
    }

    let read = get_policy(&emu, &survivor).await;
    assert_eq!(
        read.status, 200,
        "case \"the survivor\": a policy no row deleted must still read back, got {} {:?}",
        read.status, read.body
    );
}

// ---------------------------------------------------------------------------------------
// 2. A versioned ARN removes only that version.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_versioned_arn_removes_only_that_version() {
    // Rows differ along how many versions the policy has cut and which of them the delete
    // addresses — the only one, the first, a middle one and the last.
    struct Row {
        case: &'static str,
        held: u32,
        victim: u32,
    }

    let rows = [
        Row {
            case: "the only version of its policy",
            held: 1,
            victim: 1,
        },
        Row {
            case: "the first of three",
            held: 3,
            victim: 1,
        },
        Row {
            case: "a middle version of three",
            held: 3,
            victim: 2,
        },
        Row {
            case: "the last of three",
            held: 3,
            victim: 3,
        },
    ];

    let emu = emulator();

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("versioned policy {index}");
        let (policy_arn, hash) = policy(&emu, case, json!({ "name": name })).await;
        for number in 1..=row.held {
            let cut = create_version(&emu, &policy_arn, &hash).await;
            assert_eq!(
                cut.status, 200,
                "case {case:?}: cutting version {number} must answer 200, got {} {:?}",
                cut.status, cut.body
            );
        }

        // What every revision reads as BEFORE the delete, so the assertion afterwards is
        // against what the store held rather than against a hand-written expectation.
        let draft_before = get_policy(&emu, &policy_arn).await.body;
        let mut survivors: Vec<(u32, Value)> = Vec::new();
        for number in (1..=row.held).filter(|number| *number != row.victim) {
            let arn = format!("{policy_arn}:{number}");
            let read = get_policy(&emu, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: version {number} must read back before the delete, got {} {:?}",
                read.status, read.body
            );
            survivors.push((number, read.body));
        }

        let victim_arn = format!("{policy_arn}:{}", row.victim);
        assert_accepted(case, &delete_policy(&emu, &victim_arn, Force::Omitted).await);

        assert_refused(
            &format!("{case}, the deleted version"),
            &get_policy(&emu, &victim_arn).await,
            404,
            "ResourceNotFoundException",
            POLICY_VERSION_NOT_FOUND,
        );
        for (number, before) in &survivors {
            let after = get_policy(&emu, &format!("{policy_arn}:{number}")).await;
            assert_eq!(
                after.body, *before,
                "case {case:?}: the delete must leave version {number} exactly as it was"
            );
        }
        assert_eq!(
            get_policy(&emu, &policy_arn).await.body,
            draft_before,
            "case {case:?}: the delete of a version must leave the draft exactly as it was"
        );

        // A version number is never minted twice, so the next cut carries the number after
        // the highest this policy ever cut rather than filling the gap the delete left.
        let cut = create_version(&emu, &policy_arn, &hash).await;
        assert_eq!(
            cut.status, 200,
            "case {case:?}: cutting another version must answer 200, got {} {:?}",
            cut.status, cut.body
        );
        assert_eq!(
            cut.get("version"),
            json!((row.held + 1).to_string()),
            "case {case:?}: the next cut must carry the number after the highest cut so far, \
             got {:?}",
            cut.body
        );

        // Deleting the same version again has nothing to remove.
        assert_refused(
            &format!("{case}, deleted twice"),
            &delete_policy(&emu, &victim_arn, Force::True).await,
            404,
            "ResourceNotFoundException",
            POLICY_VERSION_NOT_FOUND,
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. An unforced delete is refused while the policy still holds an artifact.
// ---------------------------------------------------------------------------------------

/// Create a test under `policy_arn` and hand back its `testCaseId`.
async fn seed_test_case(emu: &Emulator, case: &str, policy_arn: &str) -> String {
    let created = create_test_case(
        emu,
        policy_arn,
        json!({
            "guardContent": "the claim under test",
            "expectedAggregatedFindingsResult": CHECK_RESULTS[0],
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the test create must answer 200, got {} {:?}",
        created.status, created.body
    );
    created.text("testCaseId")
}

#[tokio::test]
async fn an_unforced_delete_is_refused_while_the_policy_holds_an_artifact() {
    // One row per kind of artifact a policy can hold — the kinds `force` names (a policy
    // version, a test, a test result) and the build workflow a result is produced by — each
    // with the artifact kind the refusal must name. A test result only exists under a build
    // workflow, so the policy of that row holds a workflow too and the refusal names the
    // workflow: the kind a caller must delete FIRST, because deleting it takes the results it
    // produced with it.
    let rows = [
        ("policy versions", "policy version"),
        ("test cases", "test case"),
        ("build workflows", "build workflow"),
        ("test results", "build workflow"),
    ];

    let emu = emulator();

    for (index, (kind, expected_noun)) in rows.iter().enumerate() {
        let case = *kind;
        let (policy_arn, hash) =
            policy(&emu, case, json!({ "name": format!("artifact policy {index}") })).await;

        // The artifact of this row, created through the operation that creates it, and the
        // read that must stop finding it once the policy is forced away.
        let mut test_case_id = String::new();
        match *kind {
            "policy versions" => {
                let cut = create_version(&emu, &policy_arn, &hash).await;
                assert_eq!(
                    cut.status, 200,
                    "case {case:?}: cutting a version must answer 200, got {} {:?}",
                    cut.status, cut.body
                );
            }
            "test cases" => {
                test_case_id = seed_test_case(&emu, case, &policy_arn).await;
            }
            "build workflows" => {
                seed_build_workflow(&emu, &policy_arn, INGEST_CONTENT, LONG_BUILT, |_| {});
            }
            _ => {
                let build_workflow_id =
                    seed_build_workflow(&emu, &policy_arn, INGEST_CONTENT, LONG_BUILT, |_| {});
                test_case_id = seed_test_case(&emu, case, &policy_arn).await;
                let run = emu
                    .call(
                        "POST",
                        &format!(
                            "/automated-reasoning-policies/{}/build-workflows/\
                             {build_workflow_id}/test-workflows",
                            label(&policy_arn)
                        ),
                        Some(json!({ "testCaseIds": [test_case_id.clone()] })),
                    )
                    .await;
                assert_eq!(
                    run.status, 200,
                    "case {case:?}: starting the test workflow must answer 200, got {} {:?}",
                    run.status, run.body
                );
            }
        }

        // Neither spelling of an unforced delete may take the policy while it holds that
        // artifact, and the refusal says which kind is in the way.
        for unforced in [Force::Omitted, Force::False] {
            let refused = delete_policy(&emu, &policy_arn, unforced).await;
            let spelled = format!("{case}, force {unforced:?}");
            assert_eq!(
                (refused.status, refused.error.as_deref()),
                (400, Some("ValidationException")),
                "case {spelled:?}: a policy still holding an artifact must be refused, got {} \
                 {:?}",
                refused.status,
                refused.body
            );
            let message = message_of(&spelled, &refused);
            assert_eq!(
                message,
                format!("There are {expected_noun} that have not been deleted."),
                "case {spelled:?}: the refusal must be the service's sentence naming the \
                 artifact kind in the way, got {message:?}"
            );
            let read = get_policy(&emu, &policy_arn).await;
            assert_eq!(
                read.status, 200,
                "case {spelled:?}: a refused delete must leave the policy readable, got {} {:?}",
                read.status, read.body
            );
        }

        // Forced, the policy goes and its artifacts go with it: nothing reads the test the
        // policy held, and the policy itself is gone.
        assert_accepted(
            &format!("{case}, forced"),
            &delete_policy(&emu, &policy_arn, Force::True).await,
        );
        assert_refused(
            &format!("{case}, forced, read back"),
            &get_policy(&emu, &policy_arn).await,
            404,
            "ResourceNotFoundException",
            POLICY_NOT_FOUND_ON_READ,
        );
        if !test_case_id.is_empty() {
            let orphan = get_test_case(&emu, &policy_arn, &test_case_id).await;
            assert_eq!(
                (orphan.status, orphan.error.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: the test of a deleted policy must be unreachable, got {} {:?}",
                orphan.status,
                orphan.body
            );
        }
    }
}

#[tokio::test]
async fn an_unforced_delete_takes_a_policy_whose_artifacts_are_gone() {
    let emu = emulator();
    let (policy_arn, hash) = policy(
        &emu,
        "the emptied policy",
        json!({ "name": "the emptied policy" }),
    )
    .await;
    let cut = create_version(&emu, &policy_arn, &hash).await;
    assert_eq!(
        cut.status, 200,
        "case setup: cutting a version must answer 200, got {} {:?}",
        cut.status, cut.body
    );

    let refused = delete_policy(&emu, &policy_arn, Force::False).await;
    assert_eq!(
        (refused.status, refused.error.as_deref()),
        (400, Some("ValidationException")),
        "case \"holding a version\": the unforced delete must be refused, got {} {:?}",
        refused.status,
        refused.body
    );

    // Deleting the version itself is what the refusal asks for, and the unforced delete
    // then goes through: the validation is about what the policy STILL holds.
    assert_accepted(
        "the version in the way",
        &delete_policy(&emu, &format!("{policy_arn}:1"), Force::False).await,
    );
    assert_accepted(
        "the emptied policy",
        &delete_policy(&emu, &policy_arn, Force::False).await,
    );
    assert_refused(
        "the emptied policy, read back",
        &get_policy(&emu, &policy_arn).await,
        404,
        "ResourceNotFoundException",
        POLICY_NOT_FOUND_ON_READ,
    );
}

// ---------------------------------------------------------------------------------------
// 4. An ARN naming no revision is refused.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_naming_no_revision_is_refused() {
    let emu = emulator();
    let (live, hash) = policy(&emu, "the live policy", json!({ "name": "the live policy" })).await;
    let cut = create_version(&emu, &live, &hash).await;
    assert_eq!(
        cut.status, 200,
        "case setup: cutting a version must answer 200, got {} {:?}",
        cut.status, cut.body
    );

    // Rows differ along WHICH part of a well-formed `AutomatedReasoningPolicyArn` names
    // nothing: the policy id, the region, or the version number — and the sentence follows
    // from that, since a version of a policy that IS there is reported as the missing
    // revision rather than as a missing policy.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a policy id nothing minted",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/zzzzzzzzzzzz"),
            POLICY_NOT_FOUND,
        ),
        (
            "the live policy addressed in another region",
            live.replace(REGION, "eu-west-1"),
            POLICY_NOT_FOUND,
        ),
        (
            "a versioned ARN of a policy nothing minted",
            format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/zzzzzzzzzzzz:1"
            ),
            POLICY_NOT_FOUND,
        ),
        (
            "a version this policy has not cut",
            format!("{live}:2"),
            POLICY_VERSION_NOT_FOUND,
        ),
        (
            "a version number far past the ones cut",
            format!("{live}:4294967295"),
            POLICY_VERSION_NOT_FOUND,
        ),
    ];

    for (case, policy_arn, message) in &rows {
        // Forcing a delete does not conjure a policy, so both spellings are refused the
        // same way.
        for force in [Force::Omitted, Force::True] {
            let spelled = format!("{case}, force {force:?}");
            assert_refused(
                &spelled,
                &delete_policy(&emu, policy_arn, force).await,
                404,
                "ResourceNotFoundException",
                message,
            );
        }
    }

    // Every row above was refused for the ARN it carried rather than for the request it
    // was: the revisions that ARE there still read back.
    for (case, arn) in [("the draft", live.clone()), ("version 1", format!("{live}:1"))] {
        let read = get_policy(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the revision must still read back, got {} {:?}",
            read.status, read.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. A policy a guardrail checks against is in use by it.
// ---------------------------------------------------------------------------------------

/// `CreateGuardrail` carrying an `automatedReasoningPolicyConfig` that names `policies`, and
/// the members the model requires of every guardrail.
async fn guardrail_checking(emu: &Emulator, name: &str, policies: &[String]) -> String {
    let created = emu
        .create(json!({
            "name": name,
            "blockedInputMessaging": "blocked in",
            "blockedOutputsMessaging": "blocked out",
            "automatedReasoningPolicyConfig": { "policies": policies },
        }))
        .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: CreateGuardrail must answer 202, got {} {:?}",
        created.status, created.body
    );
    created.text("guardrailId")
}

#[tokio::test]
async fn a_policy_a_guardrail_checks_against_is_in_use() {
    let emu = emulator();
    // A policy the rows' bystander guardrail checks against, so each row is refused for the
    // reference its OWN guardrail holds rather than for any guardrail holding one.
    let (unchecked, _) = policy(
        &emu,
        "the unchecked policy",
        json!({ "name": "the unchecked policy" }),
    )
    .await;

    // Rows differ along which revision the guardrail names and which revision the delete
    // addresses: a delete of the POLICY takes every revision with it, so a guardrail naming
    // any of them is using what the call would remove, while a delete of ONE version is in
    // the way only of a guardrail naming exactly it.
    struct Row {
        case: &'static str,
        /// `None` names the policy's unversioned ARN, `Some(n)` the ARN of version `n`.
        referenced: Option<u32>,
        deleted: Option<u32>,
        in_use: bool,
    }

    let rows = [
        Row {
            case: "the draft referenced, the policy deleted",
            referenced: None,
            deleted: None,
            in_use: true,
        },
        Row {
            case: "a version referenced, the policy deleted",
            referenced: Some(1),
            deleted: None,
            in_use: true,
        },
        Row {
            case: "a version referenced, that version deleted",
            referenced: Some(1),
            deleted: Some(1),
            in_use: true,
        },
        Row {
            case: "the draft referenced, a version deleted",
            referenced: None,
            deleted: Some(1),
            in_use: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (checked, hash) = policy(
            &emu,
            case,
            json!({ "name": format!("checked policy {index}") }),
        )
        .await;
        let cut = create_version(&emu, &checked, &hash).await;
        assert_eq!(
            cut.status, 200,
            "case {case:?}: cutting a version must answer 200, got {} {:?}",
            cut.status, cut.body
        );

        let revision = |version: Option<u32>| match version {
            None => checked.clone(),
            Some(number) => format!("{checked}:{number}"),
        };
        // `GuardrailName` is `^[0-9a-zA-Z-_]+$`, so the row's index names its guardrails.
        let guardrail = guardrail_checking(
            &emu,
            &format!("checking-guardrail-{index}"),
            &[revision(row.referenced)],
        )
        .await;
        let bystander = guardrail_checking(
            &emu,
            &format!("bystander-guardrail-{index}"),
            std::slice::from_ref(&unchecked),
        )
        .await;
        let addressed = revision(row.deleted);

        if row.in_use {
            // Forcing the delete does not override another resource's use of the policy:
            // `force` governs the policy's OWN artifacts.
            for force in [Force::Omitted, Force::True] {
                let spelled = format!("{case}, force {force:?}");
                let refused = delete_policy(&emu, &addressed, force).await;
                assert_eq!(
                    (refused.status, refused.error.as_deref()),
                    (400, Some("ResourceInUseException")),
                    "case {spelled:?}: a revision a guardrail checks against must be refused, \
                     got {} {:?}",
                    refused.status,
                    refused.body
                );
                let message = message_of(&spelled, &refused);
                assert!(
                    message.contains(&guardrail),
                    "case {spelled:?}: the refusal must name the guardrail using it, got \
                     {message:?}"
                );
            }
            let read = get_policy(&emu, &addressed).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: a refused delete must leave the revision readable, got {} {:?}",
                read.status, read.body
            );

            // Once nothing references it, the same delete goes through.
            let dropped = emu.delete(&guardrail, None).await;
            assert_eq!(
                dropped.status, 202,
                "case {case:?}: DeleteGuardrail must answer 202, got {} {:?}",
                dropped.status, dropped.body
            );
            assert_accepted(case, &delete_policy(&emu, &addressed, Force::True).await);
        } else {
            // The guardrail checks against the draft and this delete takes a version, which
            // nothing names: it goes through while the guardrail stands.
            assert_accepted(case, &delete_policy(&emu, &addressed, Force::Omitted).await);
            let dropped = emu.delete(&guardrail, None).await;
            assert_eq!(
                dropped.status, 202,
                "case {case:?}: DeleteGuardrail must answer 202, got {} {:?}",
                dropped.status, dropped.body
            );
        }

        let dropped = emu.delete(&bystander, None).await;
        assert_eq!(
            dropped.status, 202,
            "case {case:?}: DeleteGuardrail must answer 202 for the bystander, got {} {:?}",
            dropped.status, dropped.body
        );
    }

    // The policy only the bystanders ever checked against is still there, so no row above
    // removed anything it did not address.
    let read = get_policy(&emu, &unchecked).await;
    assert_eq!(
        read.status, 200,
        "case \"the unchecked policy\": it must still read back, got {} {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_policy_a_published_guardrail_version_checks_against_is_in_use() {
    let emu = emulator();
    let (checked, _) = policy(
        &emu,
        "the published check",
        json!({ "name": "the published check" }),
    )
    .await;
    let guardrail =
        guardrail_checking(&emu, "publishing-guardrail", std::slice::from_ref(&checked)).await;

    // The version freezes the configuration that names the policy; the update then replaces
    // the draft's whole policy set, so the DRAFT no longer names it.
    let published = emu.create_version(&guardrail, json!({})).await;
    assert_eq!(
        published.status, 202,
        "case setup: CreateGuardrailVersion must answer 202, got {} {:?}",
        published.status, published.body
    );
    let updated = emu
        .update(
            &guardrail,
            json!({
                "name": "publishing-guardrail",
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            }),
        )
        .await;
    assert_eq!(
        updated.status, 202,
        "case setup: UpdateGuardrail must answer 202, got {} {:?}",
        updated.status, updated.body
    );

    let refused = delete_policy(&emu, &checked, Force::True).await;
    assert_eq!(
        (refused.status, refused.error.as_deref()),
        (400, Some("ResourceInUseException")),
        "case \"a published version checks against it\": the delete must be refused, got {} {:?}",
        refused.status,
        refused.body
    );

    // Dropping the guardrail drops the version that froze the reference with it.
    let dropped = emu.delete(&guardrail, None).await;
    assert_eq!(
        dropped.status, 202,
        "case \"the guardrail\": DeleteGuardrail must answer 202, got {} {:?}",
        dropped.status, dropped.body
    );
    assert_accepted(
        "nothing references it any more",
        &delete_policy(&emu, &checked, Force::Omitted).await,
    );
}
