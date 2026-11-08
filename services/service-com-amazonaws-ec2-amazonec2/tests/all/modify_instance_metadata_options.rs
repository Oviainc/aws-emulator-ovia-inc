// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyInstanceMetadataOptions`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=ModifyInstanceMetadataOptions`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the call WRITES EXACTLY THE OPTIONS THE REQUEST NAMES onto
//! the one instance it names and leaves every option it does not name as it stands ("if
//! this parameter is not specified, the existing state is maintained"), and that a change
//! it asks for is `pending` until the clock has carried it onto the instance and `applied`
//! from then on — read back through `DescribeInstances`, which reports the same stored
//! record.
//!
//! So the tables below span the CLASS rather than one member: every option of the
//! `InstanceMetadataOptionsResponse` set is modified on an instance whose launch set all
//! five to their non-default values, each hop limit of the published range's two bounds is
//! asked for, every one is asked for at once, and a request that names none is asked too.
//!
//! Four suites:
//!
//! 1. the option table — one modify per row, the answer it gives, and the record
//!    `DescribeInstances` reports once the clock has carried the change onto the instance;
//! 2. the clock — the options a modify has just set are `pending` and reach `applied` one
//!    [`TRANSITION_DELAY`] later whether or not anybody calls again;
//! 3. the lifecycle table — which instance states admit a modify ("a running or stopped
//!    instance") and which answer `IncorrectInstanceState`;
//! 4. the refusal table — the instance ids, the hop limits outside the published range and
//!    the `DryRun` probe it refuses, each asserted to leave the instance's options exactly
//!    where they were.

use std::time::Duration;

use ec2::ops::types::{
    HTTP_TOKENS_STATE_VALUES, INSTANCE_METADATA_ENDPOINT_STATE_VALUES,
    INSTANCE_METADATA_PROTOCOL_STATE_VALUES, INSTANCE_METADATA_TAGS_STATE_VALUES,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, HTTP_TOKENS_OPTIONAL, HTTP_TOKENS_REQUIRED,
    InstanceStateName, MAX_METADATA_HOP_LIMIT, METADATA_OPTIONS_STATE_APPLIED,
    METADATA_OPTIONS_STATE_PENDING, MIN_METADATA_HOP_LIMIT, TRANSITION_DELAY, invalid_id_message,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
use super::wire::{child_text, document};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here names.
const AMI: &str = "ami-0123456789abcdef0";

/// `InstanceMetadataEndpointState`'s, `InstanceMetadataProtocolState`'s and
/// `InstanceMetadataTagsState`'s `enabled` member — the word all three catalogs share
/// (`smithy_query(query="shape",shape="InstanceMetadataEndpointState")`), pinned against
/// those catalogs by [`the_words_the_rows_use_are_members_of_the_modelled_catalogs`].
const ENABLED: &str = "enabled";

/// The same three catalogs' `disabled` member.
const DISABLED: &str = "disabled";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// The five metadata options one instance carries, as every report of them spells them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    http_tokens: String,
    hop_limit: String,
    http_endpoint: String,
    http_protocol_ipv6: String,
    instance_metadata_tags: String,
}

impl Options {
    /// The options the launches below ask for: each member set to the value that is NOT its
    /// documented default, so an option a modify must leave alone cannot pass by being
    /// re-derived from the defaults.
    fn launched() -> Self {
        Self {
            http_tokens: HTTP_TOKENS_REQUIRED.to_string(),
            hop_limit: "3".to_string(),
            http_endpoint: DISABLED.to_string(),
            http_protocol_ipv6: ENABLED.to_string(),
            instance_metadata_tags: ENABLED.to_string(),
        }
    }

    /// The `MetadataOptions.*` launch parameters that ask for these options.
    fn launch_params(&self) -> Vec<(String, String)> {
        vec![
            p("MetadataOptions.HttpTokens", self.http_tokens.clone()),
            p(
                "MetadataOptions.HttpPutResponseHopLimit",
                self.hop_limit.clone(),
            ),
            p("MetadataOptions.HttpEndpoint", self.http_endpoint.clone()),
            p(
                "MetadataOptions.HttpProtocolIpv6",
                self.http_protocol_ipv6.clone(),
            ),
            p(
                "MetadataOptions.InstanceMetadataTags",
                self.instance_metadata_tags.clone(),
            ),
        ]
    }
}

/// The options and the `State` one `<{element}>` of a response body carries.
fn options_of(case: &str, body: &str, element: &str) -> (Options, String) {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == element)
        .unwrap_or_else(|| panic!("case {case:?}: the body carries no <{element}>: {body}"));
    let text = |member: &str| {
        child_text(node, member).unwrap_or_else(|| {
            panic!("case {case:?}: <{element}> carries no <{member}>: {body}")
        })
    };
    (
        Options {
            http_tokens: text("httpTokens"),
            hop_limit: text("httpPutResponseHopLimit"),
            http_endpoint: text("httpEndpoint"),
            http_protocol_ipv6: text("httpProtocolIpv6"),
            instance_metadata_tags: text("instanceMetadataTags"),
        },
        text("state"),
    )
}

/// Launch one instance whose metadata options are [`Options::launched`], and answer its id.
async fn launch(router: &MultiServiceRouter, case: &str) -> String {
    let mut params = vec![p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")];
    params.extend(Options::launched().launch_params());
    let (status, body) = ec2_query::call(router, REGION, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from RunInstances, got {status} with body {body}"
    );
    ec2_query::element(&body, "instanceId").unwrap_or_else(|| {
        panic!("case {case:?}: RunInstances must report the instance it minted: {body}")
    })
}

/// One `ModifyInstanceMetadataOptions` call for `instance_id` with `params` beside it.
async fn modify(
    router: &MultiServiceRouter,
    instance_id: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut all = vec![p("InstanceId", instance_id)];
    all.extend_from_slice(params);
    ec2_query::call(router, REGION, "ModifyInstanceMetadataOptions", &all).await
}

/// The options `DescribeInstances` reports for one instance, beside their `State`.
async fn described(router: &MultiServiceRouter, case: &str, instance_id: &str) -> (Options, String) {
    let (status, body) = ec2_query::call(
        router,
        REGION,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeInstances, got {status} with body {body}"
    );
    options_of(case, &body, "metadataOptions")
}

/// One row of the option table: what the request names, and what the instance's options
/// must be afterwards.
struct Row {
    /// What this row is a case of, named in every assertion of it.
    case: &'static str,
    /// The `ModifyInstanceMetadataOptions` parameters beside `InstanceId`.
    names: &'static [(&'static str, &'static str)],
    /// The options the instance must carry once the modify is served, as a mutation of
    /// [`Options::launched`].
    expected: fn(Options) -> Options,
    /// Whether the request asks for a change the clock has to carry onto the instance, so
    /// the answer reports `pending` rather than the `applied` the options already stood in.
    changes: bool,
}

/// Every option of the set, one at a time, the two bounds of the published hop-limit range,
/// all five at once, and a request that names none.
const ROWS: &[Row] = &[
    Row {
        case: "HttpTokens back to optional",
        names: &[("HttpTokens", HTTP_TOKENS_OPTIONAL)],
        expected: |launched| Options {
            http_tokens: HTTP_TOKENS_OPTIONAL.to_string(),
            ..launched
        },
        changes: true,
    },
    Row {
        case: "HttpEndpoint back on",
        names: &[("HttpEndpoint", ENABLED)],
        expected: |launched| Options {
            http_endpoint: ENABLED.to_string(),
            ..launched
        },
        changes: true,
    },
    Row {
        case: "HttpProtocolIpv6 off",
        names: &[("HttpProtocolIpv6", DISABLED)],
        expected: |launched| Options {
            http_protocol_ipv6: DISABLED.to_string(),
            ..launched
        },
        changes: true,
    },
    Row {
        case: "InstanceMetadataTags off",
        names: &[("InstanceMetadataTags", DISABLED)],
        expected: |launched| Options {
            instance_metadata_tags: DISABLED.to_string(),
            ..launched
        },
        changes: true,
    },
    Row {
        case: "the hop limit at the bottom of the published range",
        names: &[("HttpPutResponseHopLimit", "1")],
        expected: |launched| Options {
            hop_limit: "1".to_string(),
            ..launched
        },
        changes: true,
    },
    Row {
        case: "the hop limit at the top of the published range",
        names: &[("HttpPutResponseHopLimit", "64")],
        expected: |launched| Options {
            hop_limit: "64".to_string(),
            ..launched
        },
        changes: true,
    },
    Row {
        case: "the hop limit inside the published range",
        names: &[("HttpPutResponseHopLimit", "16")],
        expected: |launched| Options {
            hop_limit: "16".to_string(),
            ..launched
        },
        changes: true,
    },
    Row {
        case: "every option at once",
        names: &[
            ("HttpTokens", HTTP_TOKENS_OPTIONAL),
            ("HttpPutResponseHopLimit", "8"),
            ("HttpEndpoint", ENABLED),
            ("HttpProtocolIpv6", DISABLED),
            ("InstanceMetadataTags", DISABLED),
        ],
        expected: |_launched| Options {
            http_tokens: HTTP_TOKENS_OPTIONAL.to_string(),
            hop_limit: "8".to_string(),
            http_endpoint: ENABLED.to_string(),
            http_protocol_ipv6: DISABLED.to_string(),
            instance_metadata_tags: DISABLED.to_string(),
        },
        changes: true,
    },
    Row {
        case: "no option at all",
        names: &[],
        expected: |launched| launched,
        changes: false,
    },
];

/// The parameters one row names.
fn named(names: &[(&str, &str)]) -> Vec<(String, String)> {
    names.iter().map(|(key, value)| p(key, *value)).collect()
}

/// Rule 1: the call writes exactly the options the request names, leaves every other one as
/// it stands, answers with the instance's own id and those options, and `DescribeInstances`
/// reports the same record.
#[tokio::test]
async fn writes_the_options_the_request_names_and_maintains_the_rest() {
    let (router, _engine) = ec2_query::emulator();

    // One instance per row, all launched with the same non-default options.
    let mut instances = Vec::new();
    for row in ROWS {
        instances.push(launch(&router, row.case).await);
    }
    // The options a launch set are pending until the clock carries them on, so every row
    // starts from the same applied record.
    past_the_transition().await;

    for (row, instance_id) in ROWS.iter().zip(&instances) {
        let case = row.case;
        let expected = (row.expected)(Options::launched());

        let (status, body) = modify(&router, instance_id, &named(row.names)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the modify, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::element(&body, "instanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: the answer must name the instance it modified: {body}"
        );
        let (answered, state) = options_of(case, &body, "instanceMetadataOptions");
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the answer to report {expected:?}, got {answered:?}"
        );
        let expected_state = if row.changes {
            METADATA_OPTIONS_STATE_PENDING
        } else {
            METADATA_OPTIONS_STATE_APPLIED
        };
        assert_eq!(
            state, expected_state,
            "case {case:?}: expected the answer's state {expected_state:?}, got {state:?}"
        );
    }

    // The move the clock owes each change falls due without anybody calling again.
    past_the_transition().await;

    for (row, instance_id) in ROWS.iter().zip(&instances) {
        let case = row.case;
        let expected = (row.expected)(Options::launched());
        let (reported, state) = described(&router, case, instance_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected DescribeInstances to report {expected:?}, got {reported:?}"
        );
        assert_eq!(
            state, METADATA_OPTIONS_STATE_APPLIED,
            "case {case:?}: expected the options to be {METADATA_OPTIONS_STATE_APPLIED:?} once \
             the clock carried them on, got {state:?}"
        );
    }
}

/// Rule 2: the words the rows above ask for are members of the catalog of the member they
/// are asked for, so no row can pin a value the model does not publish.
#[test]
fn the_words_the_rows_use_are_members_of_the_modelled_catalogs() {
    for (member, catalog, words) in [
        (
            "HttpTokens",
            HTTP_TOKENS_STATE_VALUES,
            [HTTP_TOKENS_OPTIONAL, HTTP_TOKENS_REQUIRED],
        ),
        (
            "HttpEndpoint",
            INSTANCE_METADATA_ENDPOINT_STATE_VALUES,
            [ENABLED, DISABLED],
        ),
        (
            "HttpProtocolIpv6",
            INSTANCE_METADATA_PROTOCOL_STATE_VALUES,
            [ENABLED, DISABLED],
        ),
        (
            "InstanceMetadataTags",
            INSTANCE_METADATA_TAGS_STATE_VALUES,
            [ENABLED, DISABLED],
        ),
    ] {
        for word in words {
            assert!(
                catalog.contains(&word),
                "case {member:?}: expected the catalog {catalog:?} to publish {word:?}, it does \
                 not"
            );
        }
    }
    assert_eq!(
        (MIN_METADATA_HOP_LIMIT, MAX_METADATA_HOP_LIMIT),
        (1, 64),
        "case \"the published hop-limit range\": expected the documented 1-64, got {:?}",
        (MIN_METADATA_HOP_LIMIT, MAX_METADATA_HOP_LIMIT)
    );
}

/// Rule 3: "modify the instance metadata parameters on a running or stopped instance" — an
/// instance on its way to `terminated`, or already there, answers `IncorrectInstanceState`
/// and keeps its options; every other state admits the modify.
#[tokio::test]
async fn only_an_instance_that_is_not_terminating_admits_a_modify() {
    let states = [
        (InstanceStateName::Pending, true),
        (InstanceStateName::Running, true),
        (InstanceStateName::Stopping, true),
        (InstanceStateName::Stopped, true),
        (InstanceStateName::ShuttingDown, false),
        (InstanceStateName::Terminated, false),
    ];
    for (state, admitted) in states {
        let case = format!("a modify of an instance in {}", state.as_str());
        let (router, engine) = ec2_query::emulator();
        let instance_id = launch(&router, &case).await;
        {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let instance = guard.instances.get_mut(&instance_id).unwrap_or_else(|| {
                panic!("case {case:?}: the store must hold the launched instance {instance_id}")
            });
            instance.state = state;
            instance.pending_transition = None;
        }

        let (status, body) = modify(
            &router,
            &instance_id,
            &[p("HttpTokens", HTTP_TOKENS_OPTIONAL)],
        )
        .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let (answered, _) = options_of(&case, &body, "instanceMetadataOptions");
            assert_eq!(
                answered.http_tokens, HTTP_TOKENS_OPTIONAL,
                "case {case:?}: expected httpTokens {HTTP_TOKENS_OPTIONAL:?}, got {answered:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            "IncorrectInstanceState",
            "case {case:?}: expected the state refusal, got {body}"
        );
        assert_eq!(
            ec2_query::error_message(&body),
            format!(
                "The instance '{instance_id}' is not in a state from which it can be modified."
            ),
            "case {case:?}: expected the refusal to name the instance and the action: {body}"
        );
        let (reported, _) = described(&router, &case, &instance_id).await;
        assert_eq!(
            reported,
            Options::launched(),
            "case {case:?}: a refused modify must leave the options as they were, got \
             {reported:?}"
        );
    }
}

/// The whole sentence a hop limit outside the published range is refused with, as a
/// template of the value the request sent: the wording this operation answers, with the
/// bounds taken from the range the member publishes.
fn hop_limit_refusal(value: &str) -> String {
    format!(
        "A value of '{value}' is not valid for HttpPutResponseHopLimit. Specify a value \
         between {MIN_METADATA_HOP_LIMIT} and {MAX_METADATA_HOP_LIMIT} and try again."
    )
}

/// One row of the refusal table.
struct Refused {
    case: &'static str,
    /// The `InstanceId` the request names, or `None` for the instance this suite launched.
    instance_id: Option<&'static str>,
    /// The parameters beside it.
    names: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
    /// The whole sentence the refusal must carry, as a template of the id the request named.
    message: fn(&str) -> String,
}

/// The ids, hop limits and probes a modify refuses.
const REFUSALS: &[Refused] = &[
    Refused {
        case: "an instance id outside the form EC2 issues",
        instance_id: Some("not-an-instance"),
        names: &[("HttpTokens", HTTP_TOKENS_OPTIONAL)],
        status: 400,
        code: "InvalidInstanceID.Malformed",
        message: |named| invalid_id_message(named),
    },
    Refused {
        case: "a well-formed instance id this account holds no instance for",
        instance_id: Some("i-0fedcba9876543210"),
        names: &[("HttpTokens", HTTP_TOKENS_OPTIONAL)],
        status: 400,
        code: "InvalidInstanceID.NotFound",
        message: |named| format!("The instance ID '{named}' does not exist"),
    },
    Refused {
        case: "a hop limit below the published range",
        instance_id: None,
        names: &[("HttpPutResponseHopLimit", "0")],
        status: 400,
        code: "InvalidParameterValue",
        message: |_named| hop_limit_refusal("0"),
    },
    Refused {
        case: "a hop limit above the published range",
        instance_id: None,
        names: &[("HttpPutResponseHopLimit", "65")],
        status: 400,
        code: "InvalidParameterValue",
        message: |_named| hop_limit_refusal("65"),
    },
    Refused {
        // The account-level default takes -1 as "no preference"; this member publishes no
        // such value, so it is a hop limit outside its range like any other.
        case: "the account default's no-preference hop limit",
        instance_id: None,
        names: &[("HttpPutResponseHopLimit", "-1")],
        status: 400,
        code: "InvalidParameterValue",
        message: |_named| hop_limit_refusal("-1"),
    },
    Refused {
        case: "a dry run of a request that would have succeeded",
        instance_id: None,
        names: &[
            ("HttpTokens", HTTP_TOKENS_OPTIONAL),
            ("HttpPutResponseHopLimit", "9"),
            ("DryRun", "true"),
        ],
        status: 412,
        code: "DryRunOperation",
        message: |_named| "Request would have succeeded, but DryRun flag is set.".to_string(),
    },
];

/// Rule 4: every refusal answers its own status, code and sentence, and leaves the
/// instance's options exactly where they were.
#[tokio::test]
async fn a_refused_modify_writes_nothing() {
    let (router, _engine) = ec2_query::emulator();
    let instance_id = launch(&router, "the instance every refusal is asked about").await;
    past_the_transition().await;

    for row in REFUSALS {
        let case = row.case;
        let named_id = row.instance_id.unwrap_or(instance_id.as_str());
        let (status, body) = modify(&router, named_id, &named(row.names)).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        assert_eq!(
            ec2_query::error_code(&body),
            row.code,
            "case {case:?}: expected the code {:?}, got {body}",
            row.code
        );
        assert_eq!(
            ec2_query::error_message(&body),
            (row.message)(named_id),
            "case {case:?}: expected the sentence {:?}, got {body}",
            (row.message)(named_id)
        );

        let (reported, state) = described(&router, case, &instance_id).await;
        assert_eq!(
            reported,
            Options::launched(),
            "case {case:?}: a refused modify must leave the options as they were, got \
             {reported:?}"
        );
        assert_eq!(
            state, METADATA_OPTIONS_STATE_APPLIED,
            "case {case:?}: a refused modify must start no move of its own, got {state:?}"
        );
    }
}

/// Rule 5: the options a modify has just set are `pending` on the instance report too —
/// "the metadata options are being updated and the instance is not ready to process
/// metadata traffic with the new selection" — so a caller reading straight after the call
/// sees that transient state, and the move to `applied` falls due one
/// [`TRANSITION_DELAY`] later whether or not anybody calls again.
#[tokio::test]
async fn a_change_is_pending_until_the_clock_carries_it_onto_the_instance() {
    let case = "the clock of one modify";
    let (router, _engine) = ec2_query::emulator();
    let instance_id = launch(&router, case).await;
    past_the_transition().await;

    let (status, body) = modify(
        &router,
        &instance_id,
        &[p("HttpTokens", HTTP_TOKENS_OPTIONAL)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the modify, got {status} with body {body}"
    );

    let (fresh, fresh_state) = described(&router, case, &instance_id).await;
    assert_eq!(
        fresh_state, METADATA_OPTIONS_STATE_PENDING,
        "case {case:?}: the options of a modify just served are still \
         {METADATA_OPTIONS_STATE_PENDING:?}, got {fresh_state:?}"
    );
    assert_eq!(
        fresh.http_tokens, HTTP_TOKENS_OPTIONAL,
        "case {case:?}: the new selection is reported while it is still pending, got {fresh:?}"
    );

    past_the_transition().await;
    let (settled, settled_state) = described(&router, case, &instance_id).await;
    assert_eq!(
        settled_state, METADATA_OPTIONS_STATE_APPLIED,
        "case {case:?}: one TRANSITION_DELAY after the modify the options must be \
         {METADATA_OPTIONS_STATE_APPLIED:?}, got {settled_state:?}"
    );
    assert_eq!(
        settled.http_tokens, HTTP_TOKENS_OPTIONAL,
        "case {case:?}: the applied options are the ones the modify wrote, got {settled:?}"
    );
}

/// Rule 6: nothing but the one instance the request names is touched — a second instance of
/// the same account keeps the options its own launch chose.
#[tokio::test]
async fn a_modify_touches_only_the_instance_it_names() {
    let case = "two instances, one modified";
    let (router, _engine) = ec2_query::emulator();
    let modified = launch(&router, case).await;
    let untouched = launch(&router, case).await;
    past_the_transition().await;

    let (status, body) = modify(
        &router,
        &modified,
        &[p("HttpTokens", HTTP_TOKENS_OPTIONAL), p("HttpEndpoint", ENABLED)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the modify, got {status} with body {body}"
    );
    past_the_transition().await;

    let (reported, _) = described(&router, case, &untouched).await;
    assert_eq!(
        reported,
        Options::launched(),
        "case {case:?}: the instance the request did not name keeps its own options, got \
         {reported:?}"
    );
    let (changed, _) = described(&router, case, &modified).await;
    assert_eq!(
        changed,
        Options {
            http_tokens: HTTP_TOKENS_OPTIONAL.to_string(),
            http_endpoint: ENABLED.to_string(),
            ..Options::launched()
        },
        "case {case:?}: the instance the request named carries the modified options, got \
         {changed:?}"
    );
}
