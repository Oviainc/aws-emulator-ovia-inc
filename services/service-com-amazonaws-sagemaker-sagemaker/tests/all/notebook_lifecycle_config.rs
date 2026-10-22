// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's notebook instance lifecycle configurations:
//! `CreateNotebookInstanceLifecycleConfig` and the `DescribeNotebookInstanceLifecycleConfig`
//! that reads one back.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness
//! (`Emulator`, `error_code`, `message`, `str_field`) is the crate's one wire harness,
//! shared from `inference_deployment_lifecycle`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every name shape and region, the create/describe round trip for every
//! combination of the two optional hook lists, the timestamps a create records, the
//! refusal of a name already in use and of a name nothing is stored under, and the
//! modelled bounds on the hooks' `Content` and on the hook lists' length.

use serde_json::{json, Value};

use super::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// The ARN resource type SageMaker gives a notebook instance lifecycle configuration
/// (AWS ARN reference: `notebook-instance-lifecycle-config/<name>`).
const ARN_KIND: &str = "notebook-instance-lifecycle-config";

/// The modelled bounds on `NotebookInstanceLifecycleHook$Content`
/// (`smithy_query(query="shape",shape="NotebookInstanceLifecycleConfigContent")`:
/// `length 1..16384`, `pattern ^[\S\s]+$`).
const CONTENT_MIN_LEN: usize = 1;
/// See [`CONTENT_MIN_LEN`].
const CONTENT_MAX_LEN: usize = 16_384;

/// The modelled bound on `NotebookInstanceLifecycleConfigName`
/// (`smithy_query(query="operation",operation="CreateNotebookInstanceLifecycleConfig")`:
/// `length 0..63`).
const NAME_MAX_LEN: usize = 63;

/// A `Content` value of exactly `len` bytes. `A` is in the base64 alphabet, so the value
/// is a well-formed (if trivial) base64 payload of the size the case asks for.
fn content_of_len(len: usize) -> String {
    "A".repeat(len)
}

/// A name of exactly `len` bytes satisfying the modelled pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`.
fn name_of_len(len: usize) -> String {
    "n".repeat(len)
}

/// The `Content` of the single hook `member` carries, or `None` when the member is absent.
fn hook_content(body: &Value, member: &str) -> Option<String> {
    let hooks = body.get(member)?.as_array()?;
    let first = hooks.first()?;
    Some(str_field(first, "Content"))
}

/// RULE: a create mints the resource's ARN as
/// `arn:aws:sagemaker:<signed region>:<account>:notebook-instance-lifecycle-config/<name>`,
/// for every name the model admits and whatever region the caller signed for — and the
/// emulator's default region when the request carries no usable credential scope.
#[tokio::test]
async fn created_lifecycle_config_arns_name_the_signed_region_and_the_stored_name() {
    // Rows differ along the name's shape (one character, digits, hyphen runs, mixed case
    // and the modelled 63-byte upper bound) and along the signed region.
    let names = [
        "a".to_string(),
        "9".to_string(),
        "install-pkgs".to_string(),
        "On-Start--2".to_string(),
        name_of_len(NAME_MAX_LEN),
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in &names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");

            let (status, body) = emulator
                .call_in(
                    region,
                    "CreateNotebookInstanceLifecycleConfig",
                    &json!({ "NotebookInstanceLifecycleConfigName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {body}"
            );
            assert_eq!(
                str_field(&body, "NotebookInstanceLifecycleConfigArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{ARN_KIND}/{name}"),
                "case {case}: NotebookInstanceLifecycleConfigArn, got {body:?}"
            );

            // The describe reports the same ARN the create minted, so the two agree on
            // the resource's identity.
            let described = emulator
                .call_in(
                    region,
                    "DescribeNotebookInstanceLifecycleConfig",
                    &json!({ "NotebookInstanceLifecycleConfigName": name }),
                )
                .await;
            assert_eq!(
                described.0, 200,
                "case {case}: describe after create, got {} with {}",
                described.0, described.1
            );
            assert_eq!(
                str_field(&described.1, "NotebookInstanceLifecycleConfigArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{ARN_KIND}/{name}"),
                "case {case}: described ARN, got {:?}",
                described.1
            );
        }
    }

    // No credential scope: the ARN carries the emulator's default region.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateNotebookInstanceLifecycleConfig",
            &json!({ "NotebookInstanceLifecycleConfigName": "unsigned-config" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected HTTP 200, got {status} with {body}"
    );
    assert_eq!(
        str_field(&body, "NotebookInstanceLifecycleConfigArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:{ARN_KIND}/unsigned-config"),
        "case unsigned: the ARN must fall back to the default region, got {body:?}"
    );
}

/// RULE: `DescribeNotebookInstanceLifecycleConfig` replays exactly what the create
/// stored — each optional hook list present with the `Content` it was given, or absent
/// when the create did not carry it — and it reports the name and the two timestamps.
#[tokio::test]
async fn describe_replays_exactly_what_create_stored() {
    // Rows span the cross product of the two optional hook lists, and within it the
    // content's size: absent, both bounds of the modelled length, and an ordinary script.
    let cases: Vec<(&str, Option<String>, Option<String>)> = vec![
        ("no-hooks", None, None),
        (
            "on-create-only",
            Some("IyEvYmluL2Jhc2gKZWNobyBjcmVhdGU=".to_string()),
            None,
        ),
        (
            "on-start-only",
            None,
            Some("IyEvYmluL2Jhc2gKZWNobyBzdGFydA==".to_string()),
        ),
        (
            "both-hooks",
            Some("IyEvYmluL2Jhc2gKZWNobyBjcmVhdGU=".to_string()),
            Some("IyEvYmluL2Jhc2gKZWNobyBzdGFydA==".to_string()),
        ),
        (
            "content-at-min-length",
            Some(content_of_len(CONTENT_MIN_LEN)),
            Some(content_of_len(CONTENT_MIN_LEN)),
        ),
        (
            "content-at-max-length",
            Some(content_of_len(CONTENT_MAX_LEN)),
            None,
        ),
    ];

    for (case, on_create, on_start) in cases {
        let emulator = Emulator::new();
        let mut body = json!({ "NotebookInstanceLifecycleConfigName": case });
        if let Some(content) = &on_create {
            body["OnCreate"] = json!([{ "Content": content }]);
        }
        if let Some(content) = &on_start {
            body["OnStart"] = json!([{ "Content": content }]);
        }

        let created = emulator
            .ok("CreateNotebookInstanceLifecycleConfig", &body)
            .await;
        let described = emulator
            .ok(
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": case }),
            )
            .await;

        assert_eq!(
            str_field(&described, "NotebookInstanceLifecycleConfigName"),
            case,
            "case {case}: NotebookInstanceLifecycleConfigName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "NotebookInstanceLifecycleConfigArn"),
            str_field(&created, "NotebookInstanceLifecycleConfigArn"),
            "case {case}: the described ARN must be the one the create minted, \
             got {described:?}"
        );
        assert_eq!(
            hook_content(&described, "OnCreate"),
            on_create,
            "case {case}: OnCreate, got {described:?}"
        );
        assert_eq!(
            hook_content(&described, "OnStart"),
            on_start,
            "case {case}: OnStart, got {described:?}"
        );

        // A freshly created configuration has never been modified, so the two
        // timestamps are the same instant.
        let creation_time = described
            .get("CreationTime")
            .unwrap_or_else(|| panic!("case {case}: expected a CreationTime in {described:?}"));
        let last_modified = described
            .get("LastModifiedTime")
            .unwrap_or_else(|| panic!("case {case}: expected a LastModifiedTime in {described:?}"));
        assert_eq!(
            creation_time, last_modified,
            "case {case}: a never-updated configuration must report one instant for both \
             timestamps, got CreationTime {creation_time} and LastModifiedTime {last_modified}"
        );
        assert!(
            creation_time.as_f64().is_some_and(|secs| secs > 0.0),
            "case {case}: expected a CreationTime after the epoch, got {creation_time}"
        );

        // A read changes nothing a later caller sees: the second describe is identical.
        let again = emulator
            .ok(
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": case }),
            )
            .await;
        assert_eq!(
            again, described,
            "case {case}: describing twice must answer the same thing, got {again:?}"
        );
    }
}

/// RULE: the name is the resource's identity, so a create under a name already in use is
/// refused with HTTP 400 and leaves the stored configuration untouched.
#[tokio::test]
async fn a_create_under_a_name_already_in_use_is_refused_and_changes_nothing() {
    // Rows differ in how the second create's hooks relate to the first's: adding,
    // replacing and clearing them must all be refused.
    let second_attempts: Vec<(&str, Value)> = vec![
        ("same-body", json!({ "OnCreate": [{ "Content": "Zmlyc3Q=" }] })),
        (
            "different-hook",
            json!({ "OnStart": [{ "Content": "c2Vjb25k" }] }),
        ),
        ("no-hooks", json!({})),
    ];

    for (case, extra) in second_attempts {
        let emulator = Emulator::new();
        let first = emulator
            .ok(
                "CreateNotebookInstanceLifecycleConfig",
                &json!({
                    "NotebookInstanceLifecycleConfigName": case,
                    "OnCreate": [{ "Content": "Zmlyc3Q=" }],
                }),
            )
            .await;

        let mut body = json!({ "NotebookInstanceLifecycleConfigName": case });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        let (status, parsed) = emulator
            .call("CreateNotebookInstanceLifecycleConfig", &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a name already in use must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&format!("{ACCOUNT}:{ARN_KIND}/{case}")),
            "case {case}: the refusal must name the configuration's ARN, got {parsed:?}"
        );

        // The first create's scripts survive the refused second create.
        let described = emulator
            .ok(
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": case }),
            )
            .await;
        assert_eq!(
            hook_content(&described, "OnCreate"),
            Some("Zmlyc3Q=".to_string()),
            "case {case}: the stored OnCreate must survive, got {described:?}"
        );
        assert_eq!(
            hook_content(&described, "OnStart"),
            None,
            "case {case}: the refused create must not add OnStart, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "NotebookInstanceLifecycleConfigArn"),
            str_field(&first, "NotebookInstanceLifecycleConfigArn"),
            "case {case}: the ARN must not change, got {described:?}"
        );
    }
}

/// RULE: a describe naming a configuration that is not in state is refused with HTTP 400
/// `ValidationException` naming the ARN it looked for — never a 200 with empty members,
/// and never a 501.
#[tokio::test]
async fn describing_an_absent_lifecycle_config_is_refused_with_a_400() {
    // Rows: an empty store, a store holding a different configuration, and a name whose
    // case differs from a stored one (names are identities, not case-insensitive labels).
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateNotebookInstanceLifecycleConfig",
            &json!({ "NotebookInstanceLifecycleConfigName": "Stored" }),
        )
        .await;

    for name in ["absent", "stored", "STORED", "Stored2"] {
        let (status, parsed) = emulator
            .call(
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name}: expected HTTP 400 for an absent configuration, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {name}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&format!("{ACCOUNT}:{ARN_KIND}/{name}")),
            "case {name}: the refusal must name the ARN it looked for, got {parsed:?}"
        );
    }
}

/// RULE: a request outside a modelled constraint is refused with HTTP 400 and nothing is
/// stored — the hooks' `Content` holds 1..16384 characters, each hook list holds at most
/// one hook, and the name holds at most 63 characters matching its pattern.
#[tokio::test]
async fn a_request_outside_a_modelled_bound_is_refused_and_stores_nothing() {
    // Each row sits one step outside a published bound, beside the bound itself (covered
    // by the round-trip table above).
    let cases: Vec<(&str, Value)> = vec![
        (
            "content-empty",
            json!({
                "NotebookInstanceLifecycleConfigName": "refused",
                "OnStart": [{ "Content": "" }],
            }),
        ),
        (
            "content-one-past-max",
            json!({
                "NotebookInstanceLifecycleConfigName": "refused",
                "OnStart": [{ "Content": content_of_len(CONTENT_MAX_LEN + 1) }],
            }),
        ),
        (
            "two-hooks-in-one-list",
            json!({
                "NotebookInstanceLifecycleConfigName": "refused",
                "OnCreate": [{ "Content": "Zmlyc3Q=" }, { "Content": "c2Vjb25k" }],
            }),
        ),
        (
            "name-one-past-max",
            json!({ "NotebookInstanceLifecycleConfigName": name_of_len(NAME_MAX_LEN + 1) }),
        ),
        (
            "name-outside-pattern",
            json!({ "NotebookInstanceLifecycleConfigName": "not a name" }),
        ),
    ];

    for (case, body) in cases {
        let emulator = Emulator::new();
        let (status, parsed) = emulator
            .call("CreateNotebookInstanceLifecycleConfig", &body)
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );

        // Nothing was stored, so the reader refuses the name the create named.
        let name = body["NotebookInstanceLifecycleConfigName"]
            .as_str()
            .expect("every row names a configuration");
        let (read_status, read_body) = emulator
            .call(
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": name }),
            )
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused create must store nothing, got {read_status} {read_body}"
        );
    }
}

/// RULE: a lifecycle configuration is stored per name, so configurations created under
/// different names are independent and each describe answers its own scripts.
#[tokio::test]
async fn configurations_are_independent_per_name() {
    let emulator = Emulator::new();
    // Rows differ in count and in which hook each one carries, so a shared or
    // last-write-wins store would show up as the wrong script on a later read.
    let configs = [
        ("alpha", Some("YWxwaGE="), None),
        ("beta", None, Some("YmV0YQ==")),
        ("gamma", Some("Z2FtbWEtYw=="), Some("Z2FtbWEtcw==")),
        ("delta", None, None),
    ];

    for (name, on_create, on_start) in configs {
        let mut body = json!({ "NotebookInstanceLifecycleConfigName": name });
        if let Some(content) = on_create {
            body["OnCreate"] = json!([{ "Content": content }]);
        }
        if let Some(content) = on_start {
            body["OnStart"] = json!([{ "Content": content }]);
        }
        emulator
            .ok("CreateNotebookInstanceLifecycleConfig", &body)
            .await;
    }

    for (name, on_create, on_start) in configs {
        let described = emulator
            .ok(
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": name }),
            )
            .await;
        assert_eq!(
            hook_content(&described, "OnCreate"),
            on_create.map(str::to_string),
            "case {name}: OnCreate, got {described:?}"
        );
        assert_eq!(
            hook_content(&described, "OnStart"),
            on_start.map(str::to_string),
            "case {name}: OnStart, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "NotebookInstanceLifecycleConfigName"),
            name,
            "case {name}: NotebookInstanceLifecycleConfigName, got {described:?}"
        );
    }
}

/// RULE: `Tags` is accepted up to the modelled 50 entries and is not part of this
/// resource's describe output (`DescribeNotebookInstanceLifecycleConfig` models no
/// `Tags` member), so a tagged create round-trips its scripts and reports no tags.
#[tokio::test]
async fn tags_are_accepted_up_to_the_modelled_limit_and_are_not_described() {
    // `Tags` is `TagList, len 0..50` (smithy_query operation=
    // CreateNotebookInstanceLifecycleConfig): rows sit at none, one, and the upper bound.
    for count in [0_usize, 1, 50] {
        let emulator = Emulator::new();
        let case = format!("tags={count}");
        let tags: Vec<Value> = (0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
            .collect();

        let (status, body) = emulator
            .call(
                "CreateNotebookInstanceLifecycleConfig",
                &json!({
                    "NotebookInstanceLifecycleConfigName": "tagged",
                    "OnStart": [{ "Content": "dGFnZ2Vk" }],
                    "Tags": tags,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body}"
        );

        let described = emulator
            .ok(
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": "tagged" }),
            )
            .await;
        assert_eq!(
            hook_content(&described, "OnStart"),
            Some("dGFnZ2Vk".to_string()),
            "case {case}: OnStart, got {described:?}"
        );
        assert!(
            described.get("Tags").is_none(),
            "case {case}: the describe output models no Tags member, got {described:?}"
        );
    }

    // One past the modelled 50 is refused.
    let emulator = Emulator::new();
    let too_many: Vec<Value> = (0..51)
        .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
        .collect();
    let (status, parsed) = emulator
        .call(
            "CreateNotebookInstanceLifecycleConfig",
            &json!({
                "NotebookInstanceLifecycleConfigName": "too-many-tags",
                "Tags": too_many,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case tags=51: expected HTTP 400 past the modelled TagList bound, got {status} {parsed}"
    );
}
