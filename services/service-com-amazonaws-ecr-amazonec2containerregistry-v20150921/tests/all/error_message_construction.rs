// SPDX-License-Identifier: CC0-1.0

//! How this registry CONSTRUCTS the message on a refusal, end to end over the wire.
//!
//! Every refusal below already carries the right HTTP status and error code; what these
//! suites pin is the sentence, which ECR builds from the request rather than writing out
//! per call site. Four rules, one suite each:
//!
//! 1. a resource-not-found or already-exists refusal names the KIND of record, the
//!    identifier the request addressed it by, and the registry it was looked for in;
//! 2. a layer-upload diagnostic states what a client needs to recover — the byte the
//!    upload expects next on a non-contiguous part, and nothing but the verdict on a
//!    digest that does not match;
//! 3. a parameter ECR validates ITSELF — one the request carries, and an ARN it must
//!    RESOLVE wherever it carries one — is named by its API label (not the validating
//!    walk's path to it) and its constraint stated in ECR's own words: an expression with
//!    no escape a regular expression does not need, and prose for such an ARN. (A value
//!    the model-driven walk caught — a member inside a NESTED structure, or a parameter
//!    whose value the registry only RECORDS — keeps that walk's path and wording; that
//!    half of the rule is pinned in `parameter_constraint_clauses.rs`.)
//! 4. a refusal caused by a per-resource state names the resource that refused.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over — the
//! record kind and the operation, the bytes an upload already holds, which constraint and
//! where the member sits, which repository — and the recorded instance each cause was
//! filed against is ONE row. Every case goes through the router, so each exercises
//! routing, the boundary validator, the handler, the persisted state and serialization.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, layer_digest, ok, refused, REGISTRY_ID};

/// An upload id of `UploadId`'s modelled shape
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`) that
/// this registry never issued, so it passes the boundary and resolves to nothing.
const UNISSUED_UPLOAD_ID: &str = "6f736d90-0f5c-4d1e-9a2b-2c1f8e4b7a31";

/// The error code ECR states a refused input member with: the client-input error 57 of
/// this model's 58 operations declare.
const INVALID_PARAMETER: &str = "InvalidParameterException";

// ---------------------------------------------------------------------------
// The constructions under test, written out literally — the point of the suite is that
// the service builds exactly these from the request, so computing them from the
// implementation would assert nothing.
// ---------------------------------------------------------------------------

/// `UploadNotFoundException`: the upload id, the repository it was addressed in, the
/// registry.
fn upload_not_found(upload_id: &str, repository: &str) -> String {
    format!(
        "The upload with id '{upload_id}' in the repository with name '{repository}' in \
         registry with id '{REGISTRY_ID}' does not exist"
    )
}

/// `PullThroughCacheRuleNotFoundException`: the kind of record, the prefix, the registry.
fn rule_not_found(prefix: &str) -> String {
    format!(
        "A pull-through cache rule with repository prefix '{prefix}' does not exist in \
         the registry with id '{REGISTRY_ID}'"
    )
}

/// `TemplateNotFoundException`: the kind of record, the prefix, the registry.
fn template_not_found(prefix: &str) -> String {
    format!(
        "A repository creation template with prefix '{prefix}' is not found in the \
         registry with id '{REGISTRY_ID}'"
    )
}

/// `ExclusionNotFoundException`: the principal ARN, the registry.
fn exclusion_not_found(principal_arn: &str) -> String {
    format!(
        "The pull time update exclusion for principal arn '{principal_arn}' was not \
         found in registry with id '{REGISTRY_ID}'"
    )
}

/// `ExclusionAlreadyExistsException`: the same construction as
/// [`exclusion_not_found`], with the other verdict.
fn exclusion_already_exists(principal_arn: &str) -> String {
    format!(
        "The pull time update exclusion for principal arn '{principal_arn}' already \
         exists in registry with id '{REGISTRY_ID}'"
    )
}

/// `LifecyclePolicyPreviewInProgressException`: the repository whose preview is running,
/// the registry, and what the caller has to wait for.
fn preview_in_progress(repository: &str) -> String {
    format!(
        "Lifecycle policy preview is already in progress in the repository with name \
         '{repository}' in the registry with id '{REGISTRY_ID}', please wait until it \
         completes to start a new one"
    )
}

/// The whole `InvalidParameterException` clause: the member at fault and the constraint
/// it failed.
fn constraint_clause(member: &str, requirement: &str) -> String {
    format!("Invalid parameter at '{member}' failed to satisfy constraint: '{requirement}'")
}

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

/// `len` layer bytes that depend on `seed`: a deterministic spread over the whole byte
/// range, so two uploads do not carry the same bytes.
fn layer_bytes(seed: usize, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + seed * 97 + 13) % 256).unwrap_or_default())
        .collect()
}

/// An `UploadLayerPart` request body. A blob travels base64-encoded on the awsJson1_1
/// wire, with the codec `aws_smithy_types::base64` the wire serializer itself uses.
fn part_request(
    repository: &str,
    upload_id: &str,
    part_first_byte: i64,
    part_last_byte: i64,
    bytes: &[u8],
) -> Value {
    json!({
        "repositoryName": repository,
        "uploadId": upload_id,
        "partFirstByte": part_first_byte,
        "partLastByte": part_last_byte,
        "layerPartBlob": Value::String(aws_smithy_types::base64::encode(bytes)),
    })
}

/// Create the repository `name` unless it is already there, open an upload against it and
/// stream `held` bytes into it, answering the upload id and the bytes it now holds.
async fn given_upload_holding(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    seed: usize,
    held: usize,
) -> (String, Vec<u8>) {
    // A repository is created once per name; a case that reuses one does not care which
    // of its uploads created it.
    call(router, "CreateRepository", &json!({"repositoryName": name})).await;
    let opened = ok(
        router,
        case,
        "InitiateLayerUpload",
        &json!({"repositoryName": name}),
    )
    .await;
    let upload_id = opened["uploadId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: InitiateLayerUpload must hand back an uploadId"))
        .to_string();
    let bytes = layer_bytes(seed, held);
    if !bytes.is_empty() {
        let last = i64::try_from(bytes.len()).expect("a test layer fits in an i64") - 1;
        ok(
            router,
            case,
            "UploadLayerPart",
            &part_request(name, &upload_id, 0, last, &bytes),
        )
        .await;
    }
    (upload_id, bytes)
}

/// A lifecycle policy document that reaches `LifecyclePolicyText`'s modelled minimum
/// length of 100 characters and that this registry's policy reader accepts.
fn policy_text() -> String {
    let mut text = json!({
        "rules": [{
            "rulePriority": 1,
            "selection": {
                "tagStatus": "untagged",
                "countType": "imageCountMoreThan",
                "countNumber": 1,
            },
            "action": {"type": "expire"},
        }],
    })
    .to_string();
    // `@length(min: 100, max: 30720)`: trailing space is insignificant to the document
    // and is the cheapest way to reach the bound.
    while text.len() < 100 {
        text.push(' ');
    }
    text
}

// ---------------------------------------------------------------------------
// Cause 1: not-found and already-exists
// ---------------------------------------------------------------------------

/// One refusal row: the case, the operation, the request, the error code and the whole
/// message the answer must carry.
type MessageRow = (String, &'static str, Value, &'static str, String);

/// A refusal about a record the registry does not hold — or already holds — names the
/// KIND of record, the identifier the request addressed it by, and the registry it was
/// looked for in.
///
/// The rows differ along what the rule generalises over: which kind of record is missing
/// (layer upload, pull through cache rule, repository creation template, pull time update
/// exclusion), which operation asked for it, how the identifier is written (a prefix with
/// and without the trailing slash its documentation calls assumed; a role ARN and a user
/// ARN), and whether the verdict is "not found" or "already exists".
#[tokio::test]
async fn a_refusal_about_a_record_names_the_record_the_request_addressed() {
    let router = emulator();

    // A completed upload's id resolves to nothing, which is one of the states
    // `UploadNotFoundException` reports; the other is an id for another repository.
    let (spent_upload, content) = given_upload_holding(&router, "a spent upload", "spent", 1, 32)
        .await;
    ok(
        &router,
        "a spent upload",
        "CompleteLayerUpload",
        &json!({
            "repositoryName": "spent",
            "uploadId": spent_upload,
            "layerDigests": [layer_digest(&content)],
        }),
    )
    .await;
    let (other_upload, _) = given_upload_holding(&router, "an upload of another repository", "elsewhere", 2, 16).await;
    call(&router, "CreateRepository", &json!({"repositoryName": "asking"})).await;

    // One exclusion is registered, so the second register of the same principal is the
    // already-exists half of the same construction.
    const HELD_ROLE: &str = "arn:aws:iam::000000000000:role/already-excluded";
    const HELD_USER: &str = "arn:aws:iam::000000000000:user/also-excluded";
    for held in [HELD_ROLE, HELD_USER] {
        ok(
            &router,
            "the exclusion under test",
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": held}),
        )
        .await;
    }

    let rows: Vec<MessageRow> = vec![
        (
            "an upload id a completion already closed".to_string(),
            "CompleteLayerUpload",
            json!({
                "repositoryName": "spent",
                "uploadId": spent_upload,
                "layerDigests": [layer_digest(&content)],
            }),
            "UploadNotFoundException",
            upload_not_found(&spent_upload, "spent"),
        ),
        (
            "an upload id this registry never issued".to_string(),
            "UploadLayerPart",
            part_request("asking", UNISSUED_UPLOAD_ID, 0, 3, &layer_bytes(3, 4)),
            "UploadNotFoundException",
            upload_not_found(UNISSUED_UPLOAD_ID, "asking"),
        ),
        (
            "an upload opened against another repository".to_string(),
            "CompleteLayerUpload",
            json!({
                "repositoryName": "asking",
                "uploadId": other_upload,
                "layerDigests": [layer_digest(&layer_bytes(2, 16))],
            }),
            "UploadNotFoundException",
            upload_not_found(&other_upload, "asking"),
        ),
        (
            "a delete of a pull through cache rule nothing created".to_string(),
            "DeletePullThroughCacheRule",
            json!({"ecrRepositoryPrefix": "no-such-rule"}),
            "PullThroughCacheRuleNotFoundException",
            rule_not_found("no-such-rule"),
        ),
        (
            "the same prefix written with its assumed trailing slash".to_string(),
            "DescribePullThroughCacheRules",
            json!({"ecrRepositoryPrefixes": ["no-such-rule/"]}),
            "PullThroughCacheRuleNotFoundException",
            rule_not_found("no-such-rule/"),
        ),
        (
            "an update of a pull through cache rule nothing created".to_string(),
            "UpdatePullThroughCacheRule",
            json!({
                "ecrRepositoryPrefix": "absent-rule",
                "customRoleArn": "arn:aws:iam::000000000000:role/pull-through",
            }),
            "PullThroughCacheRuleNotFoundException",
            rule_not_found("absent-rule"),
        ),
        (
            "a delete of a repository creation template nothing created".to_string(),
            "DeleteRepositoryCreationTemplate",
            json!({"prefix": "no-such-template"}),
            "TemplateNotFoundException",
            template_not_found("no-such-template"),
        ),
        (
            "an update of a repository creation template nothing created".to_string(),
            "UpdateRepositoryCreationTemplate",
            json!({"prefix": "absent-template", "description": "nothing to update"}),
            "TemplateNotFoundException",
            template_not_found("absent-template"),
        ),
        (
            "a deregister of a role nothing excluded".to_string(),
            "DeregisterPullTimeUpdateExclusion",
            json!({"principalArn": "arn:aws:iam::000000000000:role/never-excluded"}),
            "ExclusionNotFoundException",
            exclusion_not_found("arn:aws:iam::000000000000:role/never-excluded"),
        ),
        (
            "a deregister of a user nothing excluded".to_string(),
            "DeregisterPullTimeUpdateExclusion",
            json!({"principalArn": "arn:aws:iam::000000000000:user/never-excluded"}),
            "ExclusionNotFoundException",
            exclusion_not_found("arn:aws:iam::000000000000:user/never-excluded"),
        ),
        (
            "a second register of an excluded role".to_string(),
            "RegisterPullTimeUpdateExclusion",
            json!({"principalArn": HELD_ROLE}),
            "ExclusionAlreadyExistsException",
            exclusion_already_exists(HELD_ROLE),
        ),
        (
            "a second register of an excluded user".to_string(),
            "RegisterPullTimeUpdateExclusion",
            json!({"principalArn": HELD_USER}),
            "ExclusionAlreadyExistsException",
            exclusion_already_exists(HELD_USER),
        ),
    ];

    for (case, operation, request, code, expected) in rows {
        let body = refused(&router, &case, operation, &request, code).await;
        let message = body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 2: layer-upload diagnostics
// ---------------------------------------------------------------------------

/// A part that does not begin where the upload's received bytes end is refused with a
/// message naming the byte the part DID begin at and the byte it had to begin at — the
/// offset a resuming client continues from — while `lastValidByteReceived` keeps
/// reporting the last byte the upload holds.
///
/// The rows differ along what the rule generalises over: how many bytes the upload
/// already holds (none, a few, a hundred and twenty-eight) and how the offset misses it (a
/// gap of one, a gap of many, a replay of bytes already received, an overlap).
#[tokio::test]
async fn a_non_contiguous_part_names_the_byte_the_upload_expects_next() {
    let router = emulator();

    // (case, bytes the upload already holds, the first byte the part declares)
    let rows: Vec<(&str, usize, i64)> = vec![
        ("an empty upload and a part starting at byte 1", 0, 1),
        ("an empty upload and a part starting far in", 0, 4096),
        ("ten bytes held and a gap of one", 10, 11),
        ("ten bytes held and a replay from byte 0", 10, 0),
        ("ten bytes held and an overlap of one", 10, 9),
        ("a hundred and twenty-eight bytes held and a gap of eight", 128, 136),
        ("a hundred and twenty-eight bytes held and a gap of many", 128, 1_000),
    ];

    for (index, (case, held, first)) in rows.into_iter().enumerate() {
        let repository = format!("resume-{index}");
        let (upload_id, _) = given_upload_holding(&router, case, &repository, index, held).await;
        // The byte the upload expects next is where its received bytes end, which is the
        // count of them: an upload holding nothing expects byte 0.
        let expected_first = i64::try_from(held).expect("a test byte count fits in an i64");
        let last_valid = expected_first.checked_sub(1).filter(|last| *last >= 0);

        let body = refused(
            &router,
            case,
            "UploadLayerPart",
            &part_request(&repository, &upload_id, first, first + 3, &layer_bytes(index, 4)),
            "InvalidLayerPartException",
        )
        .await;
        let message = body["message"].as_str().unwrap_or_default();
        let expected = format!("First byte of the layer part is {first} instead of {expected_first}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        assert_eq!(
            body["lastValidByteReceived"].as_i64(),
            last_valid,
            "case {case:?}: expected lastValidByteReceived {last_valid:?}, got {:?}",
            body["lastValidByteReceived"]
        );
    }
}

/// A completion whose claimed digest is not the digest the registry calculated is refused
/// with the verdict alone: not one of the digests offered matched. The answer echoes
/// neither the digests the request offered nor the one the registry calculated.
///
/// The rows differ along what the rule generalises over: how the claim misses (the digest
/// of other bytes, the digest of a prefix of the bytes received, a digest nothing hashes
/// to) and how many digests the request offers.
#[tokio::test]
async fn a_digest_that_does_not_match_is_refused_with_the_verdict_alone() {
    /// The one sentence ECR answers a digest mismatch with, whatever was claimed.
    const VERDICT: &str = "None of the provided layer digests match the calculated layer digest";

    let router = emulator();
    let held = layer_bytes(7, 64);
    let calculated = layer_digest(&held);
    let absent = format!("sha256:{}", "0".repeat(64));

    let rows: Vec<(&str, Vec<String>)> = vec![
        ("the digest of other bytes", vec![layer_digest(&layer_bytes(8, 64))]),
        ("the digest of a prefix of the bytes received", vec![layer_digest(&held[..32])]),
        ("a digest nothing hashes to", vec![absent.clone()]),
        (
            "two digests, neither of them the calculated one",
            vec![absent.clone(), layer_digest(&layer_bytes(9, 8))],
        ),
    ];

    for (index, (case, claimed)) in rows.into_iter().enumerate() {
        let repository = format!("mismatch-{index}");
        let (upload_id, _) = given_upload_holding(&router, case, &repository, 7, 64).await;
        let body = refused(
            &router,
            case,
            "CompleteLayerUpload",
            &json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": claimed,
            }),
            "InvalidLayerException",
        )
        .await;
        let message = body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message, VERDICT,
            "case {case:?}: expected the message {VERDICT:?}, got {message:?}"
        );
        assert!(
            !message.contains(&calculated),
            "case {case:?}: the refusal must not echo the calculated digest {calculated}, got \
             {message:?}"
        );
        for digest in &claimed {
            assert!(
                !message.contains(digest.as_str()),
                "case {case:?}: the refusal must not echo the claimed digest {digest}, got \
                 {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Cause 3: parameter-constraint clauses
// ---------------------------------------------------------------------------

/// The `repositoryName` expression as AWS states it: `RepositoryName`'s `@pattern`
/// without the `^` / `$` the model anchors with, and with the namespace separator bare —
/// `/` is no regular-expression metacharacter, so `\/` and `/` match the same character.
///
/// `smithy_query(query="shape", shape="RepositoryName", model_path="services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json")`
/// gives `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$`.
const REPOSITORY_NAME_EXPRESSION: &str =
    r"[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*";

/// The same for `PullThroughCacheRuleRepositoryPrefix`, whose `@pattern` escapes two
/// slashes — one inside the namespace group and the optional trailing one (same
/// `smithy_query` call, `shape="PullThroughCacheRuleRepositoryPrefix"`).
const PREFIX_EXPRESSION: &str =
    r"([a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*/?|ROOT)";

/// A `credentialArn` long enough to clear `CredentialArn`'s `@length(50..612)` while
/// naming no `ecr-pullthroughcache/` secret, so the `@pattern` is what refuses it.
const WRONG_NAMESPACE_SECRET: &str =
    "arn:aws:secretsmanager:eu-west-1:000000000000:secret:not-the-pull-through-namespace";

/// A parameter ECR validates itself is stated as the MEMBER at fault — never the
/// validating walk's path to the value — with the constraint in ECR's own words: an ARN it
/// must RESOLVE in prose naming the value, every other `@pattern` as the expression
/// with no escape a regular expression does not need, and every other constraint
/// unchanged.
///
/// Every row is such a parameter: one the request CARRIES (a top-level member, or an
/// element of one of its lists, which is a value of that same parameter) or an ARN naming
/// a resource the operation has to reach, which ECR checks the form of wherever the
/// request carries it.
///
/// The rows differ along what the rule generalises over: which constraint failed
/// (`@pattern` on a plain string, on an ARN, `@length`, `@required`), where the member
/// sits (top level, an element of a list of strings, a member of a structure inside a
/// list — and the second element of that list, so the position cannot be in the name),
/// which member is ARN-valued, and which operation was called.
#[tokio::test]
async fn a_parameter_ecr_validates_itself_is_stated_as_aws_states_it() {
    /// One `SigningRule` naming `arn` as its signing profile.
    fn signing_rule(arn: &str) -> Value {
        json!({"signingProfileArn": arn})
    }

    let well_formed_profile =
        format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/WellFormed");
    let too_long_name = "a".repeat(257);

    // (case, operation, request, the member the clause names, the constraint it states)
    let rows: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "an uppercase letter in a repository name",
            "CreateRepository",
            json!({"repositoryName": "Invalid_Name"}),
            "repositoryName",
            format!("must satisfy regular expression '{REPOSITORY_NAME_EXPRESSION}'"),
        ),
        (
            "a repository name opening with the namespace separator",
            "CreateRepository",
            json!({"repositoryName": "/leading-separator"}),
            "repositoryName",
            format!("must satisfy regular expression '{REPOSITORY_NAME_EXPRESSION}'"),
        ),
        (
            "an element of a list of names, which is one value of that parameter",
            "DescribeRepositories",
            json!({"repositoryNames": ["well-formed", "ALSO-Upper"]}),
            "repositoryName",
            format!("must satisfy regular expression '{REPOSITORY_NAME_EXPRESSION}'"),
        ),
        (
            "a pull through cache rule prefix outside its pattern",
            "DeletePullThroughCacheRule",
            json!({"ecrRepositoryPrefix": "Not A Prefix"}),
            "ecrRepositoryPrefix",
            format!("must satisfy regular expression '{PREFIX_EXPRESSION}'"),
        ),
        (
            "an expression with no escape at all",
            "CreateRepository",
            json!({"repositoryName": "well-formed", "registryId": "00000000000"}),
            "registryId",
            "must satisfy regular expression '[0-9]{12}'".to_string(),
        ),
        (
            "the signing profile of the only rule",
            "PutSigningConfiguration",
            json!({"signingConfiguration": {
                "rules": [signing_rule("not-a-valid-signer-profile-arn")],
            }}),
            "signingProfileArn",
            "The signing profile ARN 'not-a-valid-signer-profile-arn' must be well formed"
                .to_string(),
        ),
        (
            "the signing profile of the SECOND rule",
            "PutSigningConfiguration",
            json!({"signingConfiguration": {
                "rules": [
                    signing_rule(&well_formed_profile),
                    signing_rule("arn:aws:iam::000000000000:role/NotASigningProfile"),
                ],
            }}),
            "signingProfileArn",
            "The signing profile ARN 'arn:aws:iam::000000000000:role/NotASigningProfile' must \
             be well formed"
                .to_string(),
        ),
        (
            "a credential naming a secret outside the pull through cache namespace",
            "CreatePullThroughCacheRule",
            json!({
                "ecrRepositoryPrefix": "docker-hub",
                "upstreamRegistryUrl": "registry-1.docker.io",
                "credentialArn": WRONG_NAMESPACE_SECRET,
            }),
            "credentialArn",
            format!("The credential ARN '{WRONG_NAMESPACE_SECRET}' must be well formed"),
        ),
        (
            "a length bound, which is not worded as an expression at all",
            "CreateRepository",
            json!({"repositoryName": too_long_name}),
            "repositoryName",
            "Member must have length less than or equal to 256".to_string(),
        ),
        (
            "a required member that is absent",
            "CreateRepository",
            json!({}),
            "repositoryName",
            "Member must not be null".to_string(),
        ),
    ];

    let router = emulator();
    for (case, operation, request, member, requirement) in rows {
        let body = refused(&router, case, operation, &request, INVALID_PARAMETER).await;
        let message = body["message"].as_str().unwrap_or_default();
        let expected = constraint_clause(member, &requirement);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the clause {expected:?}, got {message:?}"
        );
        // Two spellings AWS never emits for a parameter of its OWN validation: a
        // collection step out of the validating walk's path, and a backslash before a
        // slash.
        assert!(
            !message.contains(".member"),
            "case {case:?}: the clause must not carry a collection step, got {message:?}"
        );
        assert!(
            !message.contains(r"\/"),
            "case {case:?}: the clause must state a bare slash, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Cause 4: a refusal caused by a per-resource state
// ---------------------------------------------------------------------------

/// A second lifecycle policy preview started while one is still running is refused with a
/// message naming the repository whose preview is running and the registry holding it, so
/// a caller previewing many repositories at once can tell which one refused.
///
/// The rows differ along what the rule generalises over: which repository is previewed,
/// including a namespaced name — the refusal is about the repository the request named,
/// and a preview running in one repository does not refuse a start in another.
#[tokio::test]
async fn a_preview_already_running_names_the_repository_it_runs_in() {
    let router = emulator();
    let repositories = ["previewed-one", "previewed-two", "team-a/previewed"];
    let request = |name: &str| json!({"repositoryName": name, "lifecyclePolicyText": policy_text()});

    for name in repositories {
        ok(
            &router,
            name,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;
        // Start and refuse within the one row: the completion is owed on the clock, so
        // nothing of this suite may depend on a span measured across rows. A start in
        // this repository while the previous rows' previews may still be running is also
        // what shows the refusal is per repository.
        let started = ok(&router, name, "StartLifecyclePolicyPreview", &request(name)).await;
        assert_eq!(
            started["status"].as_str(),
            Some("IN_PROGRESS"),
            "case {name:?}: the preview has to be running for the second start to be refused, \
             got {started:?}"
        );
        let reading = ok(
            &router,
            name,
            "GetLifecyclePolicyPreview",
            &json!({"repositoryName": name}),
        )
        .await;
        assert_eq!(
            reading["status"].as_str(),
            Some("IN_PROGRESS"),
            "case {name:?}: the preview is reported as running, got {reading:?}"
        );

        let body = refused(
            &router,
            name,
            "StartLifecyclePolicyPreview",
            &request(name),
            "LifecyclePolicyPreviewInProgressException",
        )
        .await;
        let message = body["message"].as_str().unwrap_or_default();
        let expected = preview_in_progress(name);
        assert_eq!(
            message, expected,
            "case {name:?}: expected the message {expected:?}, got {message:?}"
        );
        for other in repositories {
            assert!(
                other == name || !message.contains(other),
                "case {name:?}: the refusal must not name {other:?}, got {message:?}"
            );
        }
    }
}
