// SPDX-License-Identifier: CC0-1.0

//! The ECS MANAGED DAEMON control plane: `CreateDaemon` and `ListDaemons`.
//!
//! The rules under test, each asserted over a table that spans its class rather than over
//! one request:
//!
//! * "ECS Managed Daemons is only supported for Amazon ECS Managed Instances Capacity
//!   Providers" — a create whose `capacityProviderArns` names ANY capacity provider whose
//!   `CapacityProvider$type` is not `MANAGED_INSTANCES` is refused with HTTP 400
//!   `InvalidParameterException`, whichever other kind the provider is and wherever in the
//!   list it sits, and the refused call leaves no daemon behind;
//! * a create over managed-instances providers alone stores the daemon: it answers the
//!   daemon's own ARN, `ACTIVE`, the instant it was created and the ARN of the initial
//!   daemon deployment, and `ListDaemons` reports that stored record back;
//! * `clientToken` — "An identifier that you provide to ensure the idempotency of the
//!   request" — so the same token on the same daemon names the daemon that is already
//!   there instead of creating a second one;
//! * `ListDaemons`: "You can filter the results by cluster or capacity provider" — a
//!   listing reports the daemons of ONE cluster, narrowed to the providers the request
//!   names, paged by `maxResults`/`nextToken`.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into the state struct and a
//! response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::capacity_providers::managed_instances;
use crate::ecs_wire::{
    create_auto_scaling_group, create_cluster, list, ok, refusal, service, string, ACCOUNT,
    SIGNED_REGION,
};

/// The sentence AWS refuses a daemon over a provider that is not a managed-instances one
/// with: the fixed text of that error class, naming nothing from the request.
const MANAGED_INSTANCES_REQUIRED: &str =
    "All capacity providers must be of MANAGED_INSTANCES type for Daemon services";

/// The longest a `daemonName` may be: "Up to 255 letters (uppercase and lowercase),
/// numbers, underscores, and hyphens are allowed" — the member's own doc.
const MAX_NAME_LENGTH: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_NAME_LENGTH;

/// The longest a `clientToken` may be: "Up to 36 ASCII characters in the range of 33-126
/// (inclusive) are allowed" — the member's own doc.
const MAX_CLIENT_TOKEN_LENGTH: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_CLIENT_TOKEN_LENGTH;

/// The largest page `ListDaemons` may be asked for: "This value can be between 1 and
/// 100" — the `maxResults` member's own doc.
const MAX_PAGE_SIZE: i32 =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_LIST_PAGE_SIZE;

/// How many capacity providers one cluster may hold, and so the longest list of them a
/// daemon call may carry.
const MAX_CLUSTER_PROVIDERS: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_CLUSTER_CAPACITY_PROVIDERS;

/// The image the daemon task definitions here run; irrelevant to every rule, so one value.
const IMAGE: &str = "public.ecr.aws/aws-observability/aws-otel-collector:latest";

/// The ARN a capacity provider of `name` carries.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/{name}")
}

/// The ARN a daemon of `name` in `cluster` carries.
fn daemon_arn(cluster: &str, name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:daemon/{cluster}/{name}")
}

/// The prefix the ARN of a daemon deployment of `name` in `cluster` carries; the id the
/// deployment itself is minted under follows it.
fn deployment_arn_prefix(cluster: &str, name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:daemon-deployment/{cluster}/{name}/")
}

/// Create a MANAGED INSTANCES capacity provider scoped to `cluster` and return its ARN.
/// "Managed instances capacity providers are cluster-scoped", so the create names one.
async fn create_managed_provider(
    router: &MultiServiceRouter,
    cluster: &str,
    name: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "CreateCapacityProvider",
        json!({
            "name": name,
            "cluster": cluster,
            "managedInstancesProvider": managed_instances(),
        }),
        case,
    )
    .await;
    string(&body["capacityProvider"], "capacityProviderArn", case)
}

/// Create a capacity provider bound onto a caller's Auto Scaling group — a provider whose
/// `type` is `EC2_AUTOSCALING` rather than the managed instances a daemon needs — and
/// return its ARN. The group is created first, through Auto Scaling's own API: a provider
/// over a group the account does not hold is refused.
async fn create_group_provider(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let group = format!("asg-{name}");
    create_auto_scaling_group(router, &group, case).await;
    let body = ok(
        router,
        "CreateCapacityProvider",
        json!({
            "name": name,
            "autoScalingGroupProvider": { "autoScalingGroupArn": group },
        }),
        case,
    )
    .await;
    string(&body["capacityProvider"], "capacityProviderArn", case)
}

/// Register one daemon task-definition revision of `family` and return its ARN.
async fn register_daemon_definition(
    router: &MultiServiceRouter,
    family: &str,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterDaemonTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [{ "name": "agent", "image": IMAGE, "memory": 128 }],
        }),
        case,
    )
    .await;
    string(&body, "daemonTaskDefinitionArn", case)
}

/// `ListDaemons` for one request, returning the summaries it reported and the token it
/// answered with.
async fn list_page(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let body = ok(router, "ListDaemons", request, case).await;
    assert!(
        body.get("daemonSummariesList").is_some(),
        "case {case:?}: daemonSummariesList is always materialised, got {body}"
    );
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (list(&body, "daemonSummariesList", case), token)
}

/// Every daemon of `cluster`, in one unpaged listing.
async fn list_daemons(router: &MultiServiceRouter, cluster: &str, case: &str) -> Vec<Value> {
    list_page(router, json!({ "clusterArn": cluster }), case).await.0
}

/// The `daemonArn` of each summary in a listing.
fn arns(summaries: &[Value], case: &str) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| string(summary, "daemonArn", case))
        .collect()
}

#[tokio::test]
async fn a_capacity_provider_that_is_not_managed_instances_is_refused_and_stores_no_daemon() {
    let router = service();
    const CLUSTER: &str = "daemon-refusals";
    create_cluster(&router, CLUSTER, "setup").await;
    let managed = create_managed_provider(&router, CLUSTER, "mi-refusals", "setup").await;
    let group = create_group_provider(&router, "asg-refusals", "setup").await;
    let definition = register_daemon_definition(&router, "daemonrefusals", "setup").await;

    // The rows differ along what the rule generalises over: WHICH kind of non-managed
    // provider the list names (a Fargate provider Amazon ECS publishes for every account,
    // one bound onto a caller's Auto Scaling group), how it is named (short name or full
    // ARN), how many entries the list carries and WHERE the offending one sits in it.
    let rows: [(&str, Vec<String>); 7] = [
        (
            "the built-in FARGATE provider by short name",
            vec!["FARGATE".to_string()],
        ),
        (
            "the built-in FARGATE provider by ARN",
            vec![provider_arn("FARGATE")],
        ),
        (
            "the built-in FARGATE_SPOT provider by ARN",
            vec![provider_arn("FARGATE_SPOT")],
        ),
        (
            "a provider bound onto an Auto Scaling group",
            vec![group.clone()],
        ),
        (
            "a managed-instances provider followed by FARGATE",
            vec![managed.clone(), provider_arn("FARGATE")],
        ),
        (
            "FARGATE followed by a managed-instances provider",
            vec![provider_arn("FARGATE"), managed.clone()],
        ),
        (
            "two providers, neither of them managed instances",
            vec![provider_arn("FARGATE_SPOT"), group.clone()],
        ),
    ];

    for (index, (case, providers)) in rows.iter().enumerate() {
        let (code, message) = refusal(
            &router,
            "CreateDaemon",
            json!({
                "clusterArn": CLUSTER,
                "daemonName": format!("refused{index}"),
                "daemonTaskDefinitionArn": definition,
                "capacityProviderArns": providers,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException for {providers:?}, got \
             {code} with {message:?}"
        );
        assert_eq!(
            message, MANAGED_INSTANCES_REQUIRED,
            "case {case:?}: expected the managed-instances refusal, got {message:?}"
        );
    }

    // "AWS must leave no daemon behind for a refused call": every create above named a
    // daemon of its own, and none of them is in the cluster.
    let listed = list_daemons(&router, CLUSTER, "after the refusals").await;
    assert!(
        listed.is_empty(),
        "expected a refused create to store no daemon, got {listed:?}"
    );

    // The control: the same create over the managed-instances provider ALONE succeeds, so
    // the refusals above are the provider's type and nothing else about the request.
    let created = ok(
        &router,
        "CreateDaemon",
        json!({
            "clusterArn": CLUSTER,
            "daemonName": "accepted",
            "daemonTaskDefinitionArn": definition,
            "capacityProviderArns": [managed],
        }),
        "the control create",
    )
    .await;
    assert_eq!(
        string(&created, "daemonArn", "the control create"),
        daemon_arn(CLUSTER, "accepted"),
        "expected the created daemon's own ARN, got {created}"
    );
    let listed = list_daemons(&router, CLUSTER, "after the control create").await;
    assert_eq!(
        arns(&listed, "after the control create"),
        vec![daemon_arn(CLUSTER, "accepted")],
        "expected the one accepted daemon to be listed, got {listed:?}"
    );
}

/// One row of the create table: the daemon's name, how many managed-instances providers
/// it deploys onto, which form of the daemon task-definition reference it names, and the
/// optional members the request carries.
struct CreateRow {
    case: &'static str,
    name: &'static str,
    providers: usize,
    /// How the `daemonTaskDefinitionArn` is spelled: `0` the full ARN, `1` the family,
    /// `2` `family:revision` — the three forms a reference to this resource takes.
    reference_form: usize,
    extra: fn() -> Value,
}

#[tokio::test]
async fn a_daemon_over_managed_instances_providers_is_stored_and_read_back_through_list_daemons() {
    let router = service();
    const CLUSTER: &str = "daemon-store";
    const FAMILY: &str = "daemonstore";
    create_cluster(&router, CLUSTER, "setup").await;
    let definition = register_daemon_definition(&router, FAMILY, "setup").await;
    // Three managed-instances providers in the cluster, so a daemon can be created over
    // one, two or all three of them.
    let mut providers = Vec::new();
    for slot in 0..3 {
        providers.push(
            create_managed_provider(&router, CLUSTER, &format!("mi-store{slot}"), "setup").await,
        );
    }

    // The rows differ along the count of capacity providers, the form of the task
    // definition reference, and the optional members the create carries.
    let rows = [
        CreateRow {
            case: "one provider and nothing optional",
            name: "plain",
            providers: 1,
            reference_form: 0,
            extra: || json!({}),
        },
        CreateRow {
            case: "two providers, tags and tag propagation",
            name: "tagged",
            providers: 2,
            reference_form: 0,
            extra: || {
                json!({
                    "tags": [{ "key": "team", "value": "platform" }],
                    "propagateTags": "DAEMON",
                    "enableECSManagedTags": true,
                })
            },
        },
        CreateRow {
            case: "three providers and a deployment configuration",
            name: "rolling",
            providers: 3,
            reference_form: 1,
            extra: || {
                json!({
                    "deploymentConfiguration": {
                        "drainPercent": 25.0,
                        "bakeTimeInMinutes": 5,
                        "alarms": { "alarmNames": ["daemon-errors"], "enable": true },
                    },
                })
            },
        },
        CreateRow {
            case: "a non-critical daemon with execute command on",
            name: "noncritical",
            providers: 1,
            reference_form: 2,
            extra: || json!({ "critical": false, "enableExecuteCommand": true }),
        },
    ];

    let mut expected: Vec<String> = Vec::new();
    let mut deployments: Vec<String> = Vec::new();
    for row in &rows {
        let case = row.case;
        let reference = match row.reference_form {
            0 => definition.clone(),
            1 => FAMILY.to_string(),
            _ => format!("{FAMILY}:1"),
        };
        let mut request = json!({
            "clusterArn": CLUSTER,
            "daemonName": row.name,
            "daemonTaskDefinitionArn": reference,
            "capacityProviderArns": providers[..row.providers],
        });
        for (member, value) in (row.extra)()
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the row's extra members are an object"))
        {
            request[member.as_str()] = value.clone();
        }

        let created = ok(&router, "CreateDaemon", request, case).await;
        let arn = string(&created, "daemonArn", case);
        assert_eq!(
            arn,
            daemon_arn(CLUSTER, row.name),
            "case {case:?}: expected the daemon's own ARN, got {created}"
        );
        assert_eq!(
            string(&created, "status", case),
            "ACTIVE",
            "case {case:?}: a created daemon is ACTIVE, got {created}"
        );
        let created_at = created
            .get("createdAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a createdAt, got {created}"));
        assert!(
            created_at > 0.0,
            "case {case:?}: expected the instant the daemon was created, got {created_at}"
        );
        let deployment = string(&created, "deploymentArn", case);
        let prefix = deployment_arn_prefix(CLUSTER, row.name);
        assert!(
            deployment.starts_with(&prefix),
            "case {case:?}: expected the initial deployment's ARN under {prefix:?}, got \
             {deployment:?}"
        );
        assert!(
            !deployments.contains(&deployment),
            "case {case:?}: every daemon deployment is distinct, but {deployment:?} was \
             minted twice"
        );
        deployments.push(deployment);
        expected.push(arn.clone());

        // The create is read back through the listing: the stored record, not the request.
        let listed = list_daemons(&router, CLUSTER, case).await;
        let summary = listed
            .iter()
            .find(|summary| string(summary, "daemonArn", case) == arn)
            .unwrap_or_else(|| panic!("case {case:?}: expected {arn:?} in {listed:?}"))
            .clone();
        assert_eq!(
            string(&summary, "status", case),
            "ACTIVE",
            "case {case:?}: expected the stored status back, got {summary}"
        );
        for member in ["createdAt", "updatedAt"] {
            let reported = summary.get(member).and_then(Value::as_f64).unwrap_or_else(|| {
                panic!("case {case:?}: expected a {member} in {summary}")
            });
            assert_eq!(
                reported, created_at,
                "case {case:?}: expected {member} to be the instant the create reported \
                 ({created_at}), got {reported}"
            );
        }
    }

    // Every daemon created above is listed, and nothing else is.
    let mut listed = arns(
        &list_daemons(&router, CLUSTER, "the whole cluster").await,
        "the whole cluster",
    );
    listed.sort();
    expected.sort();
    assert_eq!(
        listed, expected,
        "expected the listing to report exactly the created daemons, got {listed:?}"
    );
}

#[tokio::test]
async fn a_create_outside_a_documented_constraint_is_refused_and_stores_nothing() {
    let router = service();
    const CLUSTER: &str = "daemon-validation";
    const FAMILY: &str = "daemonvalidation";
    create_cluster(&router, CLUSTER, "setup").await;
    let provider = create_managed_provider(&router, CLUSTER, "mi-validation", "setup").await;
    let definition = register_daemon_definition(&router, FAMILY, "setup").await;
    // A revision whose delete has been requested: still readable, but it can run no new
    // work, so a daemon cannot be created on it.
    const DELETED_FAMILY: &str = "daemondeleted";
    register_daemon_definition(&router, DELETED_FAMILY, "setup").await;
    let deleted = ok(
        &router,
        "DeleteDaemonTaskDefinition",
        json!({ "daemonTaskDefinition": format!("{DELETED_FAMILY}:1") }),
        "setup",
    )
    .await;
    let deleted = string(&deleted, "daemonTaskDefinitionArn", "setup");

    let past_name_length = "d".repeat(MAX_NAME_LENGTH + 1);
    let longest_name = "d".repeat(MAX_NAME_LENGTH);
    let past_token_length = "t".repeat(MAX_CLIENT_TOKEN_LENGTH + 1);
    // One entry past the providers one cluster may hold.
    let past_provider_bound: Vec<String> = (0..=MAX_CLUSTER_PROVIDERS)
        .map(|slot| format!("mi-bound{slot}"))
        .collect();

    // The rows span the ways a create can miss a constraint its members document: the
    // name's own, the token's, the provider list's, the cluster, and the task definition.
    let rows: [(&str, &str, Value); 11] = [
        (
            "an empty daemonName",
            "InvalidParameterException",
            json!({ "daemonName": "" }),
        ),
        (
            "a daemonName carrying a character outside the documented set",
            "InvalidParameterException",
            json!({ "daemonName": "daemon name" }),
        ),
        (
            "a daemonName one character past the documented length",
            "InvalidParameterException",
            json!({ "daemonName": past_name_length }),
        ),
        (
            "a clientToken one character past the documented length",
            "InvalidParameterException",
            json!({ "daemonName": "tokentoolong", "clientToken": past_token_length }),
        ),
        (
            "no capacity provider at all",
            "InvalidParameterException",
            json!({ "daemonName": "noproviders", "capacityProviderArns": [] }),
        ),
        (
            "a capacity provider this account does not hold",
            "InvalidParameterException",
            json!({ "daemonName": "unknownprovider", "capacityProviderArns": ["mi-nothing"] }),
        ),
        (
            "one capacity provider past the number a cluster may hold",
            "InvalidParameterException",
            json!({
                "daemonName": "toomanyproviders",
                "capacityProviderArns": past_provider_bound,
            }),
        ),
        (
            "a cluster that was never created",
            "ClusterNotFoundException",
            json!({ "daemonName": "nocluster", "clusterArn": "daemon-missing" }),
        ),
        (
            "a daemon task definition family nothing was registered under",
            "ClientException",
            json!({ "daemonName": "unknowndefinition", "daemonTaskDefinitionArn": "neverregistered" }),
        ),
        (
            "a daemonTaskDefinitionArn that is no reference at all",
            "InvalidParameterException",
            json!({ "daemonName": "malformeddefinition", "daemonTaskDefinitionArn": "not a reference" }),
        ),
        (
            "a daemon task-definition revision whose delete was requested",
            "InvalidParameterException",
            json!({ "daemonName": "deleteddefinition", "daemonTaskDefinitionArn": deleted }),
        ),
    ];

    for (case, expected_code, overrides) in rows {
        let mut request = json!({
            "clusterArn": CLUSTER,
            "daemonName": "placeholder",
            "daemonTaskDefinitionArn": definition,
            "capacityProviderArns": [provider.clone()],
        });
        for (member, value) in overrides
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the row's members are an object"))
        {
            request[member.as_str()] = value.clone();
        }
        let (code, message) = refusal(&router, "CreateDaemon", request, case).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected a {expected_code}, got {code} with {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: expected the refusal to say what was wrong, got {message:?}"
        );
    }

    // Nothing above was stored.
    let listed = list_daemons(&router, CLUSTER, "after the refusals").await;
    assert!(
        listed.is_empty(),
        "expected every refused create to store nothing, got {listed:?}"
    );

    // The published bound itself is accepted: a name of the documented length, and a
    // token of it.
    let created = ok(
        &router,
        "CreateDaemon",
        json!({
            "clusterArn": CLUSTER,
            "daemonName": longest_name,
            "daemonTaskDefinitionArn": definition,
            "capacityProviderArns": [provider],
            "clientToken": "t".repeat(MAX_CLIENT_TOKEN_LENGTH),
        }),
        "a name and a token of the documented length",
    )
    .await;
    assert_eq!(
        string(&created, "daemonArn", "a name of the documented length"),
        daemon_arn(CLUSTER, &longest_name),
        "expected a name of the documented length to be accepted, got {created}"
    );
}

#[tokio::test]
async fn the_same_client_token_names_the_daemon_that_is_already_there() {
    let router = service();
    const CLUSTER: &str = "daemon-idempotent";
    const NAME: &str = "repeated";
    const TOKEN: &str = "token-0001";
    create_cluster(&router, CLUSTER, "setup").await;
    let provider = create_managed_provider(&router, CLUSTER, "mi-idempotent", "setup").await;
    let definition = register_daemon_definition(&router, "daemonidempotent", "setup").await;

    let request = json!({
        "clusterArn": CLUSTER,
        "daemonName": NAME,
        "daemonTaskDefinitionArn": definition,
        "capacityProviderArns": [provider.clone()],
        "clientToken": TOKEN,
    });
    let first = ok(&router, "CreateDaemon", request.clone(), "the first create").await;
    // The repeat carries the same token, so it is the SAME create: the daemon that is
    // there is reported back, member for member, and no second deployment is minted.
    let repeat = ok(&router, "CreateDaemon", request, "the repeated create").await;
    assert_eq!(
        repeat, first,
        "expected the repeat of one create to report the stored daemon unchanged, got \
         {repeat} against {first}"
    );

    // A create of the same daemon under ANOTHER token is a different create of a name
    // that is taken, so it is refused.
    for (case, token) in [
        ("another token", Some("token-0002")),
        ("no token at all", None),
    ] {
        let mut request = json!({
            "clusterArn": CLUSTER,
            "daemonName": NAME,
            "daemonTaskDefinitionArn": definition,
            "capacityProviderArns": [provider.clone()],
        });
        if let Some(token) = token {
            request["clientToken"] = json!(token);
        }
        let (code, message) = refusal(&router, "CreateDaemon", request, case).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected the taken name to be refused, got {code} with \
             {message:?}"
        );
    }

    // One daemon, however many times the create was called.
    let listed = list_daemons(&router, CLUSTER, "after the repeats").await;
    assert_eq!(
        arns(&listed, "after the repeats"),
        vec![daemon_arn(CLUSTER, NAME)],
        "expected exactly one daemon, got {listed:?}"
    );
}

#[tokio::test]
async fn a_listing_reports_one_cluster_narrowed_to_the_providers_it_names_and_pages() {
    let router = service();
    const HOME: &str = "daemon-home";
    const OTHER: &str = "daemon-other";
    create_cluster(&router, HOME, "setup").await;
    create_cluster(&router, OTHER, "setup").await;
    let definition = register_daemon_definition(&router, "daemonlisting", "setup").await;
    let first = create_managed_provider(&router, HOME, "mi-first", "setup").await;
    let second = create_managed_provider(&router, HOME, "mi-second", "setup").await;
    let elsewhere = create_managed_provider(&router, OTHER, "mi-elsewhere", "setup").await;

    // The daemons differ in which providers they deploy onto and which cluster they are
    // in, so the filters below select by what is STORED and not by call order.
    let daemons: [(&str, &str, Vec<String>); 4] = [
        ("one", HOME, vec![first.clone()]),
        ("two", HOME, vec![second.clone()]),
        ("three", HOME, vec![first.clone(), second.clone()]),
        ("four", OTHER, vec![elsewhere.clone()]),
    ];
    for (name, cluster, providers) in &daemons {
        ok(
            &router,
            "CreateDaemon",
            json!({
                "clusterArn": cluster,
                "daemonName": name,
                "daemonTaskDefinitionArn": definition,
                "capacityProviderArns": providers,
            }),
            name,
        )
        .await;
    }

    // "If you do not specify a cluster, the default cluster is assumed" — and a listing
    // reports the daemons of the cluster it names and no other's.
    let case = "the home cluster";
    let mut listed = arns(&list_daemons(&router, HOME, case).await, case);
    listed.sort();
    assert_eq!(
        listed,
        vec![
            daemon_arn(HOME, "one"),
            daemon_arn(HOME, "three"),
            daemon_arn(HOME, "two"),
        ],
        "case {case:?}: expected only the home cluster's daemons, got {listed:?}"
    );
    let case = "the other cluster";
    assert_eq!(
        arns(&list_daemons(&router, OTHER, case).await, case),
        vec![daemon_arn(OTHER, "four")],
        "case {case:?}: expected only the other cluster's daemon"
    );

    // "Only daemons associated with the specified capacity providers are returned": the
    // rows differ in how many providers the filter names and how each is spelled.
    let filters: [(&str, Vec<String>, Vec<String>); 4] = [
        (
            "one provider by ARN",
            vec![first.clone()],
            vec![daemon_arn(HOME, "one"), daemon_arn(HOME, "three")],
        ),
        (
            "the same provider by its short name",
            vec!["mi-first".to_string()],
            vec![daemon_arn(HOME, "one"), daemon_arn(HOME, "three")],
        ),
        (
            "the other provider",
            vec![second.clone()],
            vec![daemon_arn(HOME, "three"), daemon_arn(HOME, "two")],
        ),
        (
            "a provider of another cluster, which no daemon here deploys onto",
            vec![elsewhere.clone()],
            Vec::new(),
        ),
    ];
    for (case, filter, expected) in filters {
        let (summaries, _) = list_page(
            &router,
            json!({ "clusterArn": HOME, "capacityProviderArns": filter.clone() }),
            case,
        )
        .await;
        let mut reported = arns(&summaries, case);
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} for the filter {filter:?}, got \
             {reported:?}"
        );
    }

    // `maxResults`/`nextToken`: a walk of one-daemon pages reports every daemon of the
    // cluster exactly once and ends with no token.
    let case = "a one-at-a-time walk";
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..10 {
        let mut request = json!({ "clusterArn": HOME, "maxResults": 1 });
        if let Some(token) = token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let (summaries, next) = list_page(&router, request, case).await;
        assert!(
            summaries.len() <= 1,
            "case {case:?}: expected at most one summary a page, got {summaries:?}"
        );
        walked.extend(arns(&summaries, case));
        token = next;
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {case:?}: expected the walk to finish, got the token {token:?}"
    );
    walked.sort();
    assert_eq!(
        walked,
        vec![
            daemon_arn(HOME, "one"),
            daemon_arn(HOME, "three"),
            daemon_arn(HOME, "two"),
        ],
        "case {case:?}: expected the paged walk to report every daemon once, got \
         {walked:?}"
    );

    // "This value can be between 1 and 100": each published bound is accepted and one
    // past either end is refused.
    for max_results in [1, MAX_PAGE_SIZE] {
        let case = format!("maxResults {max_results}");
        let (summaries, _) = list_page(
            &router,
            json!({ "clusterArn": HOME, "maxResults": max_results }),
            &case,
        )
        .await;
        assert!(
            !summaries.is_empty(),
            "case {case:?}: expected a page of daemons, got {summaries:?}"
        );
    }
    for max_results in [0, MAX_PAGE_SIZE + 1] {
        let case = format!("maxResults {max_results}");
        let (code, message) = refusal(
            &router,
            "ListDaemons",
            json!({ "clusterArn": HOME, "maxResults": max_results }),
            &case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected a page size outside the documented range to be \
             refused, got {code} with {message:?}"
        );
    }

    // The provider filter is bounded the same way a create's list is: one entry past the
    // number of providers one cluster may hold is refused.
    let case = "a provider filter one entry past the cluster's provider bound";
    let past_bound: Vec<String> = (0..=MAX_CLUSTER_PROVIDERS)
        .map(|slot| format!("mi-filter{slot}"))
        .collect();
    let (code, message) = refusal(
        &router,
        "ListDaemons",
        json!({ "clusterArn": HOME, "capacityProviderArns": past_bound }),
        case,
    )
    .await;
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected an over-long filter to be refused, got {code} with \
         {message:?}"
    );

    // A cluster that was never created holds no daemons to list, so the listing is
    // refused rather than answering an empty list for a cluster that is not there.
    let case = "a cluster that was never created";
    let (code, message) = refusal(
        &router,
        "ListDaemons",
        json!({ "clusterArn": "daemon-missing" }),
        case,
    )
    .await;
    assert_eq!(
        code, "ClusterNotFoundException",
        "case {case:?}: expected a ClusterNotFoundException, got {code} with {message:?}"
    );
}
