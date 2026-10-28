// SPDX-License-Identifier: CC0-1.0

//! What a refused ECS request SAYS when the NAME it carries is not one ECS accepts, and
//! when an account-setting read gives a value to filter by without the name it filters —
//! driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test:
//!
//! * Every caller-supplied ECS name follows ONE rule, which each name member's own doc
//!   states word for word — "Up to 255 letters (uppercase and lowercase), numbers,
//!   underscores, and hyphens are allowed" (`CreateCluster$clusterName`,
//!   `CreateService$serviceName`, `RegisterTaskDefinition$family`,
//!   `CreateDaemon$daemonName`; `smithy_query(query="doc",shape="CreateClusterRequest",
//!   member="clusterName")`). A value outside it is refused with the operation's
//!   `InvalidParameterException`, whose message states that rule as the pattern it is. A
//!   call asked to TAKE the name adds the value it rejected, bare, after the fixed
//!   `but was ` prefix — so the caller is told which of the names it sent it must change;
//!   a call that merely READS a resource by name states the rule alone.
//! * A name INSIDE the rule is accepted and reads back, at both published length bounds.
//! * `ListAccountSettings$value`: "You must also specify an account setting name to use
//!   this parameter" — a read that gives the one without the other is refused naming the
//!   member it needs and the member that needs it.
//!
//! The tables span the ways a name can break the one rule (empty, inner spaces,
//! punctuation, a non-ASCII letter, one character past the published length) and the
//! operations that take a name; nothing here reaches into the state struct, so what a
//! write stored is read back through an implemented read.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, service, string, ACCOUNT, SIGNED_REGION};
use crate::task_lifecycle::container;

/// The longest a name may be, from the member docs quoted above.
const MAX_NAME_LENGTH: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_NAME_LENGTH;

/// The rule a refusal states, spelled as the refusal of a name a call was asked to TAKE
/// spells it: the character set the docs list and the length bounds they state.
const NAME_PATTERN: &str = r"^[a-zA-Z0-9\-_]{1,255}$";

/// The same rule as the refusal of a cluster reference a READ carries spells it — the one
/// sentence whose recorded text doubles the escape before the literal hyphen.
const READ_NAME_PATTERN: &str = r"^[a-zA-Z0-9\\-_]{1,255}$";

/// The error every name refusal takes.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The cluster the rows that need one act in, and the registered family they point at.
const CLUSTER: &str = "namerules";
const FAMILY: &str = "namerulesfamily";

/// The image a container definition starts from; irrelevant to every rule here, so one
/// value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// One row of a name table: the operation, the request it builds around the name, and the
/// subject its refusal names the member by.
type NameRow = (&'static str, fn(&str) -> Value, &'static str);

/// A router holding one cluster and one revision of [`FAMILY`], so a row is refused for
/// the NAME it carries and not for a resource that happens to be missing.
async fn fixture() -> MultiServiceRouter {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": CLUSTER }),
        "fixture",
    )
    .await;
    ok(
        &router,
        "RegisterTaskDefinition",
        json!({
            "family": FAMILY,
            "containerDefinitions": [container("web", IMAGE)],
        }),
        "fixture",
    )
    .await;
    router
}

/// The ways a name can fall outside the one rule: nothing at all, inner spaces and
/// punctuation, a letter outside ASCII, and one character past the published length.
fn names_outside_the_rule() -> Vec<(&'static str, String)> {
    vec![
        ("an empty name", String::new()),
        (
            "a name with spaces and punctuation",
            "bad cluster name !! not allowed".to_string(),
        ),
        ("a name with dots", "name.with.dots".to_string()),
        ("a name with a slash", "group/name".to_string()),
        ("a name with a non-ascii letter", "nämé".to_string()),
        (
            "a name one character past the length",
            "x".repeat(MAX_NAME_LENGTH + 1),
        ),
    ]
}

#[tokio::test]
async fn a_name_outside_the_rule_is_refused_with_the_rule_and_the_value() {
    let router = fixture().await;

    // Every implemented operation asked to TAKE a name, each paired with a request that
    // is well formed apart from the name it carries.
    let rows: [NameRow; 5] = [
        (
            "CreateCluster",
            |name| json!({ "clusterName": name }),
            "Cluster name",
        ),
        (
            "CreateService",
            |name| {
                json!({
                    "cluster": CLUSTER,
                    "serviceName": name,
                    "taskDefinition": FAMILY,
                    "desiredCount": 0,
                })
            },
            "Service name",
        ),
        (
            "RegisterTaskDefinition",
            |name| {
                json!({
                    "family": name,
                    "containerDefinitions": [container("web", IMAGE)],
                })
            },
            "Family",
        ),
        (
            // `capacityProviderArns` and `daemonTaskDefinitionArn` are required members,
            // so a row carries both: what they NAME is checked after the name itself.
            "CreateDaemon",
            |name| {
                json!({
                    "daemonName": name,
                    "daemonTaskDefinitionArn": format!("{FAMILY}:1"),
                    "capacityProviderArns": ["mi-namerules"],
                })
            },
            "Daemon name",
        ),
        (
            "RegisterDaemonTaskDefinition",
            |name| {
                json!({
                    "family": name,
                    "containerDefinitions": [
                        { "name": "agent", "image": IMAGE, "memory": 128 },
                    ],
                })
            },
            "Family",
        ),
    ];

    for (operation, build, subject) in rows {
        for (form, name) in names_outside_the_rule() {
            let case = format!("{operation} with {form}");
            let (code, message) = refusal(&router, operation, build(&name), &case).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected an {INVALID_PARAMETER}, got {code:?} with \
                 {message:?}"
            );
            assert_eq!(
                message,
                format!("{subject} must match {NAME_PATTERN}, but was {name}"),
                "case {case:?}: expected the rule and the rejected name, got {message:?}"
            );
        }
    }

    // None of the refusals created anything: the store still holds only the fixture's
    // cluster.
    let listed = ok(&router, "ListClusters", json!({}), "after the refusals").await;
    assert_eq!(
        list(&listed, "clusterArns", "after the refusals").len(),
        1,
        "case \"after the refusals\": a refused name must create nothing, got {listed}"
    );
}

#[tokio::test]
async fn a_read_of_a_name_outside_the_rule_states_the_rule_alone() {
    let router = fixture().await;

    for (form, name) in names_outside_the_rule() {
        // "The short name or full Amazon Resource Name (ARN) of the cluster": a blank
        // entry names no cluster at all, so it is read as the default cluster the member
        // documents rather than as a name that broke the rule.
        if name.trim().is_empty() {
            continue;
        }
        // Both forms the member accepts carry the same name, so both are refused for it.
        let references = [
            ("a short name".to_string(), name.clone()),
            (
                "a full ARN".to_string(),
                format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{name}"),
            ),
        ];
        for (shape, reference) in references {
            let case = format!("DescribeClusters with {form} as {shape}");
            let (code, message) = refusal(
                &router,
                "DescribeClusters",
                json!({ "clusters": [reference] }),
                &case,
            )
            .await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected an {INVALID_PARAMETER}, got {code:?} with \
                 {message:?}"
            );
            assert_eq!(
                message,
                format!("ClusterName must match {READ_NAME_PATTERN}"),
                "case {case:?}: expected the rule alone, got {message:?}"
            );
            assert!(
                !message.contains(&name),
                "case {case:?}: a read states the rule and echoes no value, got \
                 {message:?}"
            );
        }
    }

    // The blank entry the loop skips: "If you do not specify a cluster, the default
    // cluster is assumed", so an entry that names nothing is read as that cluster rather
    // than refused — and the default cluster of a store that never created one is
    // reported as the MISSING resource it is.
    for (case, entry) in [
        ("an empty entry", json!("")),
        ("an entry of only spaces", json!("   ")),
    ] {
        let described = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": [entry] }),
            case,
        )
        .await;
        assert!(
            list(&described, "clusters", case).is_empty(),
            "case {case:?}: no default cluster was created, so none is described, got \
             {described}"
        );
        let failures = list(&described, "failures", case);
        assert_eq!(
            failures.len(),
            1,
            "case {case:?}: the default cluster the entry names is reported missing, got \
             {described}"
        );
        assert!(
            string(&failures[0], "arn", case).ends_with(":cluster/default"),
            "case {case:?}: the failure names the DEFAULT cluster, got {described}"
        );
    }
}

#[tokio::test]
async fn a_name_inside_the_rule_is_stored_and_reads_back() {
    let router = service();

    // Each published bound of the rule, and a name using both separators and both cases.
    let names = [
        ("the shortest name", "a".to_string()),
        ("the longest name", "n".repeat(MAX_NAME_LENGTH)),
        (
            "both separators and both cases",
            "Mixed_Case-123".to_string(),
        ),
    ];

    for (case, name) in names {
        let created = ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": name }),
            case,
        )
        .await;
        assert_eq!(
            string(&created["cluster"], "clusterName", case),
            name,
            "case {case:?}: the created cluster keeps the name the request named: \
             {created}"
        );
        let described = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": [name.clone()] }),
            case,
        )
        .await;
        let read = &described["clusters"][0];
        assert_eq!(
            string(read, "clusterName", case),
            name,
            "case {case:?}: the stored cluster reads back under its name: {described}"
        );

        // The same rule, same bounds, on a task-definition family: what the registration
        // stored reads back through the describe.
        let registered = ok(
            &router,
            "RegisterTaskDefinition",
            json!({
                "family": name,
                "containerDefinitions": [container("web", IMAGE)],
            }),
            case,
        )
        .await;
        let arn = string(&registered["taskDefinition"], "taskDefinitionArn", case);
        let described = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": name.clone() }),
            case,
        )
        .await;
        assert_eq!(
            string(&described["taskDefinition"], "taskDefinitionArn", case),
            arn,
            "case {case:?}: the registered revision reads back by family: {described}"
        );
    }
}

#[tokio::test]
async fn an_account_setting_read_names_the_member_its_filter_requires() {
    let router = service();

    // `ListAccountSettings$value`: "You must also specify an account setting name to use
    // this parameter." The rows vary the value given and whether a principal is named, so
    // the sentence is a property of the MISSING member and of nothing else.
    let rows = [
        ("a value alone", json!({ "value": "enabled" })),
        (
            "a value with the effective-settings flag",
            json!({ "value": "disabled", "effectiveSettings": true }),
        ),
        (
            "a value with a principal",
            json!({
                "value": "enabled",
                "principalArn": format!("arn:aws:iam::{ACCOUNT}:user/emulator"),
            }),
        ),
    ];

    for (case, request) in rows {
        let (code, message) = refusal(&router, "ListAccountSettings", request, case).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected an {INVALID_PARAMETER}, got {code:?} with {message:?}"
        );
        assert_eq!(
            message, "Setting 'name' must be provided when 'value' is present",
            "case {case:?}: expected the member the filter requires to be named, got \
             {message:?}"
        );
    }

    // The same filter WITH the name it requires is served: the refusal is about the
    // missing member and not about filtering.
    let listed = ok(
        &router,
        "ListAccountSettings",
        json!({
            "name": "containerInsights",
            "value": "disabled",
            "effectiveSettings": true,
        }),
        "a value beside its name",
    )
    .await;
    let settings = list(&listed, "settings", "a value beside its name");
    assert_eq!(
        settings.len(),
        1,
        "case \"a value beside its name\": the setting whose effective value the filter \
         names is reported, got {listed}"
    );
}
