// SPDX-License-Identifier: CC0-1.0

//! WHERE each half of a refusal message comes from, end to end over the wire.
//!
//! Every refusal below already carries the right HTTP status (400) and the right error
//! code; what these suites pin is the sentence, and specifically which SOURCE each part of
//! it is taken from. Four rules, one suite each:
//!
//! 1. the lifecycle policy SCHEMA states a rule as a CHOICE among the alternative forms
//!    the format admits one in, so a rule in none of them is refused with the choice's own
//!    outcome — how many alternatives it matched — and never with the sub-constraint one
//!    alternative failed, so the report carries neither the value the rule gave nor the set
//!    an alternative admits;
//! 2. both halves of ONE kind of record's lifecycle — the verdict that the registry does
//!    not hold it and the verdict that it already does — are stated in that kind's one
//!    construction: the kind of record, the identifier the request addressed it by and the
//!    registry, with only the verdict differing;
//! 3. a pattern-constrained member whose value this registry only RECORDS is refused by
//!    the model's own constraint, so its clause states the `@pattern` the model declares,
//!    anchors and all, and does not echo the value;
//! 4. a refusal about one upload names the upload by the identifier the REQUEST addressed
//!    it by — never by a value the registry computed over the bytes it received.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over — which
//! member the choice is made on and which operation read the document, which kind of
//! record and which identifier, the shape of the malformed ARN and which operation took
//! it, the repository and the layer the upload carried — and the recorded instance each
//! cause was filed against is ONE row, marked as such. Every case goes through the router,
//! so each exercises routing, the boundary validator, the handler, the persisted state and
//! serialization.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{emulator, layer_digest, ok, refused, REGISTRY_ID};

/// The error code a refused input member carries.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The API-documented label of the parameter carrying a lifecycle document.
const LIFECYCLE_POLICY_TEXT: &str = "LifecyclePolicyText";

/// The two operations that read a lifecycle document out of that parameter, so both word
/// a refusal of it the same way.
const DOCUMENT_READERS: [&str; 2] = ["PutLifecyclePolicy", "StartLifecyclePolicyPreview"];

/// The modelled minimum length of `lifecyclePolicyText` (`@length(min: 100, max: 30720)`
/// on `LifecyclePolicyText`), past which a document reaches the handler that reads it.
const MIN_TEXT_LEN: usize = 100;

/// The whole clause a refused parameter is stated with.
fn clause(parameter: &str, constraint: &str) -> String {
    format!("Invalid parameter at '{parameter}' failed to satisfy constraint: '{constraint}'")
}

/// The clause a refused lifecycle document is stated with, for the schema validator's
/// report `report` — which the validator writes as a LINE, so the quoted span keeps the
/// newline that terminates it.
fn document_clause(report: &str) -> String {
    clause(
        LIFECYCLE_POLICY_TEXT,
        &format!("Lifecycle policy validation failure: {report}\n"),
    )
}

/// The schema's report for a rule in NONE of the alternative forms it admits one in,
/// written out literally — the point of the first suite is that the service builds exactly
/// this, so computing it from the implementation would assert nothing.
///
/// The count is how many forms there are: one per count type crossed with one per action
/// type, because each pair fixes which of a rule's conditional members it carries.
const OUTSIDE_EVERY_ALTERNATIVE: &str =
    "instance failed to match exactly one schema (matched 0 out of 4)";

/// `document` as the wire carries it: padded past [`MIN_TEXT_LEN`] with trailing
/// whitespace, which a JSON parser ignores.
fn padded(document: &str) -> String {
    let mut text = document.to_string();
    while text.len() < MIN_TEXT_LEN {
        text.push(' ');
    }
    text
}

/// A lifecycle document carrying one rule.
fn one_rule(selection: Value, action: Value) -> String {
    padded(
        &json!({"rules": [{"rulePriority": 1, "selection": selection, "action": action}]})
            .to_string(),
    )
}

/// A selection counting images, with the catch-all tag status.
fn count_selection(count_type: &str, count: i64) -> Value {
    json!({"tagStatus": "any", "countType": count_type, "countNumber": count})
}

// ---------------------------------------------------------------------------
// Cause 1: the choice among a rule's alternative forms
// ---------------------------------------------------------------------------

/// A rule the lifecycle schema admits in none of its alternative forms is refused with the
/// choice's own outcome, and that report publishes nothing of the rule: not the value a
/// member the choice is made on carried, and not the values an alternative would admit.
///
/// The rows differ along what the rule generalises over: which member put the rule outside
/// every alternative (`countType`, `countUnit`, the action's `type`, its
/// `targetStorageClass`), how it did (a value the format does not define, a member one
/// form requires and the rule leaves out, a member another form forbids and the rule
/// carries) and which operation read the document. The recorded instance is the first row.
#[tokio::test]
async fn a_rule_outside_every_alternative_form_states_only_the_choice_outcome() {
    let expire = || json!({"type": "expire"});
    // (case, the document, the values the report must not publish)
    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        (
            // The recorded instance.
            "a countType the format does not define",
            one_rule(count_selection("notARealCountType", 5), expire()),
            vec!["notARealCountType", "imageCountMoreThan", "sinceImagePushed"],
        ),
        (
            "a countType differing from a defined one in case alone is admitted, not this",
            one_rule(count_selection("imageCountMoreThanSome", 5), expire()),
            vec!["imageCountMoreThanSome"],
        ),
        (
            "a countUnit the format does not define",
            one_rule(
                json!({
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countUnit": "hours",
                    "countNumber": 5,
                }),
                expire(),
            ),
            vec!["hours", "days"],
        ),
        (
            "an age count carrying no countUnit, which its form requires",
            one_rule(count_selection("sinceImagePushed", 5), expire()),
            vec!["countUnit"],
        ),
        (
            "a count of images carrying a countUnit, which its form forbids",
            one_rule(
                json!({
                    "tagStatus": "any",
                    "countType": "imageCountMoreThan",
                    "countUnit": "days",
                    "countNumber": 5,
                }),
                expire(),
            ),
            vec!["days"],
        ),
        (
            "an action type the format does not define",
            one_rule(
                count_selection("imageCountMoreThan", 5),
                json!({"type": "delete"}),
            ),
            vec!["delete", "expire", "transition"],
        ),
        (
            "a transition carrying no targetStorageClass, which its form requires",
            one_rule(
                count_selection("imageCountMoreThan", 5),
                json!({"type": "transition"}),
            ),
            vec!["targetStorageClass"],
        ),
        (
            "a transition to a storage class the format does not define",
            one_rule(
                count_selection("imageCountMoreThan", 5),
                json!({"type": "transition", "targetStorageClass": "GLACIER"}),
            ),
            vec!["GLACIER", "ARCHIVE"],
        ),
        (
            "an expiry naming a targetStorageClass, which its form forbids",
            one_rule(
                count_selection("imageCountMoreThan", 5),
                json!({"type": "expire", "targetStorageClass": "ARCHIVE"}),
            ),
            vec!["ARCHIVE"],
        ),
    ];

    let router = emulator();
    let repository = "catalog/alternative-forms";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let expected = document_clause(OUTSIDE_EVERY_ALTERNATIVE);

    for (case, document, withheld) in rows {
        for operation in DOCUMENT_READERS {
            let case = format!("{case} on {operation}");
            let body = refused(
                &router,
                &case,
                operation,
                &json!({"repositoryName": repository, "lifecyclePolicyText": document}),
                INVALID_PARAMETER,
            )
            .await;
            let message = body["message"].as_str().unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case:?}: expected the clause {expected:?}, got {message:?}"
            );
            for published in &withheld {
                assert!(
                    !message.contains(published),
                    "case {case:?}: the choice's outcome must publish neither the rule's \
                     value nor an alternative's value set, got {published:?} in {message:?}"
                );
            }
        }
    }
}

/// The other half of the same rule: a member every alternative SHARES is checked by the
/// schema common to them all, which states the constraint that failed — so the choice's
/// outcome is not the answer to every malformed document.
///
/// The rows differ along which shared member broke which constraint: a member absent, a
/// value outside the set the format defines for it, a number under its minimum, and the
/// document's own `rules` array.
#[tokio::test]
async fn a_member_every_alternative_shares_states_its_own_constraint() {
    let expire = || json!({"type": "expire"});
    // (case, the document, the report the clause quotes)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the document's own rules member absent",
            padded(r#"{"description": "a policy naming no rules"}"#),
            r#"object has missing required properties (["rules"])"#.to_string(),
        ),
        (
            "rules holding no rule at all",
            padded(&json!({"rules": []}).to_string()),
            "array is too short: must have at least 1 elements but instance has 0 elements"
                .to_string(),
        ),
        (
            "a selection naming no countNumber",
            one_rule(
                json!({"tagStatus": "any", "countType": "imageCountMoreThan"}),
                expire(),
            ),
            r#"object has missing required properties (["countNumber"])"#.to_string(),
        ),
        (
            "a tagStatus outside the values the format defines",
            one_rule(
                json!({
                    "tagStatus": "sometimes",
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire(),
            ),
            r#"instance value ("sometimes") not found in enum (possible values: ["any","tagged","untagged"])"#
                .to_string(),
        ),
        (
            "a countNumber under the minimum the format declares",
            one_rule(count_selection("imageCountMoreThan", 0), expire()),
            "numeric instance is lower than the required minimum (minimum: 1, found: 0)"
                .to_string(),
        ),
        (
            "a rulePriority under the minimum the format declares",
            padded(
                &json!({"rules": [{
                    "rulePriority": 0,
                    "selection": count_selection("imageCountMoreThan", 5),
                    "action": {"type": "expire"},
                }]})
                .to_string(),
            ),
            "numeric instance is lower than the required minimum (minimum: 1, found: 0)"
                .to_string(),
        ),
    ];

    let router = emulator();
    let repository = "catalog/shared-members";
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;

    for (case, document, report) in rows {
        let expected = document_clause(&report);
        let body = refused(
            &router,
            case,
            "PutLifecyclePolicy",
            &json!({"repositoryName": repository, "lifecyclePolicyText": document}),
            INVALID_PARAMETER,
        )
        .await;
        let message = body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the clause {expected:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 2: one construction per kind of record
// ---------------------------------------------------------------------------

/// One kind of record's two verdicts, as the registry ANSWERED them: the request that
/// found it absent, and the request that found it already there.
struct RecordLifecycle {
    /// What the record is, for the assertion messages.
    case: &'static str,
    /// The identifier the already-there request addressed the record by.
    identifier: String,
    /// The identifier the absent request addressed.
    absent_identifier: String,
    /// The message the answer to the absent request carried.
    answered_absent: String,
    /// The message the answer to the already-there request carried.
    answered_held: String,
}

/// Both halves of a record's lifecycle are stated in that kind's own construction, and the
/// two differ only in the verdict: the kind of record, the identifier the request
/// addressed it by and the registry are the same words in each.
///
/// The rows differ along what the rule generalises over: which kind of record (a repository
/// creation template, a pull-through cache rule, a pull time update exclusion), how it is
/// identified (a namespace prefix, a principal ARN of a role and of a user) and which
/// operation reached each verdict. The recorded instance is the template's already-exists
/// verdict.
#[tokio::test]
async fn both_verdicts_about_a_record_share_that_kind_s_construction() {
    let router = emulator();
    let mut rows: Vec<RecordLifecycle> = Vec::new();

    // A repository creation template, addressed by its prefix — the recorded instance.
    for prefix in ["team-a", "team-b/nested"] {
        ok(
            &router,
            "the template under test",
            "CreateRepositoryCreationTemplate",
            &template_request(prefix),
        )
        .await;
        let held = refused(
            &router,
            "a second create of the same template prefix",
            "CreateRepositoryCreationTemplate",
            &template_request(prefix),
            "TemplateAlreadyExistsException",
        )
        .await;
        let absent_prefix = format!("{prefix}-absent");
        let absent = refused(
            &router,
            "a delete of a template nothing created",
            "DeleteRepositoryCreationTemplate",
            &json!({"prefix": absent_prefix}),
            "TemplateNotFoundException",
        )
        .await;
        rows.push(RecordLifecycle {
            case: "a repository creation template",
            identifier: prefix.to_string(),
            absent_identifier: absent_prefix.clone(),
            answered_absent: answered(&absent),
            answered_held: answered(&held),
        });
        assert_message(
            "a template already in the registry",
            &held,
            &format!(
                "A repository creation template with prefix '{prefix}' already exists in \
                 the registry with id '{REGISTRY_ID}'"
            ),
        );
        assert_message(
            "a template the registry does not hold",
            &absent,
            &format!(
                "A repository creation template with prefix '{absent_prefix}' is not found \
                 in the registry with id '{REGISTRY_ID}'"
            ),
        );
    }

    // A pull-through cache rule, addressed by its repository prefix.
    let prefix = "ecr-public-mirror";
    ok(
        &router,
        "the cache rule under test",
        "CreatePullThroughCacheRule",
        &json!({"ecrRepositoryPrefix": prefix, "upstreamRegistryUrl": "public.ecr.aws"}),
    )
    .await;
    let held = refused(
        &router,
        "a second create of the same cache rule prefix",
        "CreatePullThroughCacheRule",
        &json!({"ecrRepositoryPrefix": prefix, "upstreamRegistryUrl": "public.ecr.aws"}),
        "PullThroughCacheRuleAlreadyExistsException",
    )
    .await;
    let absent = refused(
        &router,
        "a delete of a cache rule nothing created",
        "DeletePullThroughCacheRule",
        &json!({"ecrRepositoryPrefix": "no-such-mirror"}),
        "PullThroughCacheRuleNotFoundException",
    )
    .await;
    rows.push(RecordLifecycle {
        case: "a pull-through cache rule",
        identifier: prefix.to_string(),
        absent_identifier: "no-such-mirror".to_string(),
        answered_absent: answered(&absent),
        answered_held: answered(&held),
    });
    assert_message(
        "a cache rule already in the registry",
        &held,
        &format!(
            "A pull-through cache rule with repository prefix '{prefix}' already exists in \
             the registry with id '{REGISTRY_ID}'"
        ),
    );
    assert_message(
        "a cache rule the registry does not hold",
        &absent,
        &format!(
            "A pull-through cache rule with repository prefix 'no-such-mirror' does not \
             exist in the registry with id '{REGISTRY_ID}'"
        ),
    );

    // A pull time update exclusion, addressed by the principal ARN it is keyed by.
    for principal in [
        "arn:aws:iam::000000000000:role/excluded-role",
        "arn:aws:iam::000000000000:user/excluded-user",
    ] {
        ok(
            &router,
            "the exclusion under test",
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": principal}),
        )
        .await;
        let held = refused(
            &router,
            "a second register of the same principal",
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": principal}),
            "ExclusionAlreadyExistsException",
        )
        .await;
        let absent_principal = format!("{principal}-absent");
        let absent = refused(
            &router,
            "a deregister of a principal nothing excluded",
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": absent_principal}),
            "ExclusionNotFoundException",
        )
        .await;
        rows.push(RecordLifecycle {
            case: "a pull time update exclusion",
            identifier: principal.to_string(),
            absent_identifier: absent_principal.clone(),
            answered_absent: answered(&absent),
            answered_held: answered(&held),
        });
        assert_message(
            "an exclusion already in the registry",
            &held,
            &format!(
                "The pull time update exclusion for principal arn '{principal}' already \
                 exists in registry with id '{REGISTRY_ID}'"
            ),
        );
        assert_message(
            "an exclusion the registry does not hold",
            &absent,
            &format!(
                "The pull time update exclusion for principal arn '{absent_principal}' was \
                 not found in registry with id '{REGISTRY_ID}'"
            ),
        );
    }

    // The invariant over every row, read off what the registry ANSWERED: one construction
    // per kind of record, the verdict apart. Each verdict names the kind of record, then
    // the identifier the request addressed it by, then the registry — so neither can be
    // worded from the error shape's documentation instead, and neither can be stated about
    // a record the request did not name.
    for row in rows {
        let case = row.case;
        for (verdict, answered, identifier) in [
            ("already exists", &row.answered_held, &row.identifier),
            ("not found", &row.answered_absent, &row.absent_identifier),
        ] {
            assert!(
                answered.contains(&format!("'{identifier}'")),
                "case {case:?}: the {verdict} verdict must name the record the request \
                 addressed ({identifier:?}), got {answered:?}"
            );
            assert!(
                answered.ends_with(&format!("with id '{REGISTRY_ID}'")),
                "case {case:?}: the {verdict} verdict must close on the registry holding the \
                 record, got {answered:?}"
            );
        }
        // The kind of record as each verdict names it: the words before the identifier.
        let kind_of = |answered: &str| -> String {
            answered
                .split_once(" '")
                .map_or_else(|| answered.to_string(), |(kind, _)| kind.to_string())
        };
        let held_kind = kind_of(&row.answered_held);
        let absent_kind = kind_of(&row.answered_absent);
        assert_eq!(
            held_kind, absent_kind,
            "case {case:?}: both verdicts must name the record the same way, got \
             {held_kind:?} and {absent_kind:?}"
        );
    }
}

/// The message an answer carried.
fn answered(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One repository creation template request for `prefix`.
fn template_request(prefix: &str) -> Value {
    json!({
        "prefix": prefix,
        "appliedFor": ["PULL_THROUGH_CACHE"],
        "description": "the template under test",
    })
}

/// The message a refusal carries, against the one it must carry.
fn assert_message(case: &str, body: &Value, expected: &str) {
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// Causes 3 and 4: a member whose value the registry only records
// ---------------------------------------------------------------------------

/// `PrincipalArn`'s `@pattern` as the MODEL declares it, anchors and all
/// (`smithy_query(query="shape", shape="PrincipalArn")`): the exclusion list is KEYED by a
/// principal ARN rather than resolving it to a resource, so nothing of this registry's own
/// checks its form and the model's constraint is what refuses it.
const PRINCIPAL_ARN_PATTERN: &str =
    r"^arn:aws(-[a-z]+)*:iam::[0-9]{12}:(role|user)/[\w+=,.@-]+(/[\w+=,.@-]+)*$";

/// A principal ARN outside the modelled pattern is refused with the pattern the model
/// declares, and the clause does not carry the value the request sent.
///
/// The rows differ along the shape of the malformed ARN — no ARN at all, an empty value, a
/// principal kind the pattern does not admit, a service other than the identity service, a
/// region where the pattern admits none, an account id of the wrong length, a resource with
/// no name, a character the name class excludes — crossed with both operations that take
/// the member. The recorded instances are the first row on each operation.
#[tokio::test]
async fn a_principal_arn_outside_its_pattern_states_the_modelled_pattern() {
    let rows: Vec<(&str, &str)> = vec![
        // The recorded instance: a value that is no ARN at all.
        ("no ARN at all", "not-an-arn"),
        ("an empty value", ""),
        (
            "a principal kind the pattern does not admit",
            "arn:aws:iam::000000000000:group/admins",
        ),
        (
            "another service's namespace",
            "arn:aws:sts::000000000000:role/assumed",
        ),
        (
            "a region where the pattern admits none",
            "arn:aws:iam:eu-west-1:000000000000:role/regional",
        ),
        (
            "an account id of the wrong length",
            "arn:aws:iam::00000000000:role/short-account",
        ),
        (
            "a role with no name",
            "arn:aws:iam::000000000000:role/",
        ),
        (
            "a name carrying a character the class excludes",
            "arn:aws:iam::000000000000:role/not a name",
        ),
        (
            "a partition outside the ones the pattern admits",
            "arn:azure:iam::000000000000:role/elsewhere",
        ),
    ];

    let router = emulator();
    let expected = clause(
        "principalArn",
        &format!("Member must satisfy regular expression pattern: {PRINCIPAL_ARN_PATTERN}"),
    );

    for (case, principal) in rows {
        for operation in [
            "RegisterPullTimeUpdateExclusion",
            "DeregisterPullTimeUpdateExclusion",
        ] {
            let case = format!("{case} on {operation}");
            let body = refused(
                &router,
                &case,
                operation,
                &json!({"principalArn": principal}),
                INVALID_PARAMETER,
            )
            .await;
            let message = body["message"].as_str().unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case:?}: expected the clause {expected:?}, got {message:?}"
            );
            // The model's constraint is what refused the value, and its clause states the
            // expression rather than the value: a non-empty value is never echoed back.
            if !principal.is_empty() {
                assert!(
                    !message.contains(principal),
                    "case {case:?}: the clause must not echo the rejected value, got \
                     {message:?}"
                );
            }
        }
    }

    // The positive control: a principal the pattern admits is recorded, so the refusals
    // above are about the pattern rather than about the operation being closed.
    let admitted = "arn:aws:iam::000000000000:role/path/to/admitted";
    let registered = ok(
        &router,
        "a principal the pattern admits",
        "RegisterPullTimeUpdateExclusion",
        &json!({"principalArn": admitted}),
    )
    .await;
    assert_eq!(
        registered["principalArn"].as_str(),
        Some(admitted),
        "a principal the pattern admits must be recorded, got {registered:?}"
    );
}

// ---------------------------------------------------------------------------
// Cause 5: the identifier the request addressed the upload by
// ---------------------------------------------------------------------------

/// A completion of an upload whose layer the repository already holds is refused with a
/// message naming the UPLOAD the request addressed — its id — and not the digest the
/// registry computed over the bytes it received.
///
/// The rows differ along what the rule generalises over: the repository (a plain name and a
/// namespaced one), the bytes the upload carried (so the digest differs per row) and how
/// many parts carried them.
#[tokio::test]
async fn a_layer_already_held_names_the_upload_id_the_request_sent() {
    let router = emulator();
    // (case, repository, the layer's bytes, how many parts carry them)
    let rows: Vec<(&str, &str, Vec<u8>, usize)> = vec![
        ("a single-part layer", "layers/one", vec![1_u8; 32], 1),
        ("a two-part layer", "layers/two", vec![2_u8; 64], 2),
        (
            "a namespaced repository and a longer layer",
            "team-a/layers/three",
            vec![3_u8; 300],
            3,
        ),
    ];

    for (case, repository, content, parts) in rows {
        ok(
            &router,
            case,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        // The layer is pushed once, so the repository holds it.
        let first = upload(&router, case, repository, &content, parts).await;
        ok(
            &router,
            case,
            "CompleteLayerUpload",
            &completion(repository, &first, &content),
        )
        .await;

        // A second push of the same bytes opens its OWN upload, so the id the refusal must
        // name is this one rather than the one the first push used.
        let second = upload(&router, case, repository, &content, parts).await;
        assert_ne!(
            first, second,
            "case {case:?}: the second push must open an upload of its own, got {second:?}"
        );
        let body = refused(
            &router,
            case,
            "CompleteLayerUpload",
            &completion(repository, &second, &content),
            "LayerAlreadyExistsException",
        )
        .await;
        let message = body["message"].as_str().unwrap_or_default();
        let expected = format!(
            "Layer with upload id '{second}' already exists in the repository with name \
             '{repository}' in registry with id '{REGISTRY_ID}'"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        // The two things the refusal must not be keyed on: the digest the registry
        // computed, and the id of the upload that is already closed.
        let digest = layer_digest(&content);
        assert!(
            !message.contains(&digest),
            "case {case:?}: the refusal names the upload the request addressed, not the \
             digest the registry computed, got {message:?}"
        );
        assert!(
            !message.contains(&first),
            "case {case:?}: the refusal names THIS upload, not the one already closed, got \
             {message:?}"
        );
    }
}

/// Open an upload in `repository` and send `content` to it in `parts` parts, answering the
/// upload id the registry issued.
async fn upload(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    content: &[u8],
    parts: usize,
) -> String {
    let opened = ok(
        router,
        case,
        "InitiateLayerUpload",
        &json!({"repositoryName": repository}),
    )
    .await;
    let upload_id = opened["uploadId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: InitiateLayerUpload must issue an uploadId"))
        .to_string();
    let chunk = content.len().div_ceil(parts.max(1));
    let mut first = 0_usize;
    while first < content.len() {
        let last = (first + chunk).min(content.len());
        ok(
            router,
            case,
            "UploadLayerPart",
            &json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "partFirstByte": first,
                "partLastByte": last - 1,
                "layerPartBlob": aws_smithy_types::base64::encode(&content[first..last]),
            }),
        )
        .await;
        first = last;
    }
    upload_id
}

/// The completion request for `upload_id`, asserting the digest of `content`.
fn completion(repository: &str, upload_id: &str, content: &[u8]) -> Value {
    json!({
        "repositoryName": repository,
        "uploadId": upload_id,
        "layerDigests": [layer_digest(content)],
    })
}
