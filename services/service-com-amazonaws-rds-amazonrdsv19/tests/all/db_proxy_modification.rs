// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyDBProxy`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it, and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML the
//! emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test, asserted over a TABLE spanning the class it generalises over
//! rather than over one request: a modification of a proxy carries the settings a caller
//! wants CHANGED, so each setting it states is applied to the stored proxy and each one it
//! leaves out keeps the value the proxy already holds — the two documented "new" lists,
//! `Auth` and `SecurityGroups`, REPLACING what the proxy held rather than adding to it —
//! the proxy is answered and reported as applying the change until this service has it in
//! force, a rename moves the proxy to the new name and everything computed from the name
//! with it, and a request the modification cannot be taken for — an unknown proxy, a proxy
//! in a state no modification acts in, a rename onto a name the region already holds, a
//! value outside what a member accepts — is refused and leaves the proxy exactly as it was.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_PROXY_STATUS_AVAILABLE, DB_PROXY_STATUS_CREATING, DB_PROXY_STATUS_DELETING,
    DB_PROXY_STATUS_MODIFYING, DEFAULT_AUTH_SCHEME_NONE, DEFAULT_IDLE_CLIENT_TIMEOUT,
    MAX_FREE_FORM_LEN, MAX_USER_AUTH_CONFIGS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, subtrees, text, texts, wait_for_settle,
    REGION,
};
use crate::db_proxy_target_group_readiness::read_proxy;
use crate::db_proxy_target_groups::create_proxy;

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second IAM role ARN of the form `ModifyDBProxy$RoleArn` takes, so a row can move the
/// proxy off the role its create gave it.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/rds-proxy-rotated";

/// A Secrets Manager secret ARN of the form `UserAuthConfig$SecretArn` takes — "the secret
/// that the proxy uses to authenticate to the RDS DB instance or Aurora DB cluster".
const FIRST_SECRET_ARN: &str =
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:emu-proxy-first-AbCdEf";

/// A second secret ARN, so a row can bind a different secret than the row before it.
const SECOND_SECRET_ARN: &str =
    "arn:aws:secretsmanager:us-west-2:000000000000:secret:emu-proxy-second-GhIjKl";

/// One row of the modification table: the parameters the call carries beyond the proxy
/// name, and the WHOLE of the settings the proxy must report afterwards.
struct ModifyCase {
    case: &'static str,
    /// The members the request states, beyond `DBProxyName`.
    sent: &'static [(&'static str, &'static str)],
    /// Every scalar setting the proxy reports afterwards, whether this row stated it or
    /// not — so a row proves both what it changed and what it left alone.
    reported: &'static [(&'static str, &'static str)],
    /// The `SecretArn` of each `Auth` entry the proxy reports afterwards, in order.
    auth_secret_arns: &'static [&'static str],
    /// The security group ids the proxy reports afterwards, in order.
    security_groups: &'static [&'static str],
}

/// `ModifyDBProxy` for `proxy`, with `sent` beyond the proxy's name.
async fn modify_proxy(
    router: &MultiServiceRouter,
    proxy: &str,
    sent: &[(&str, &str)],
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![("DBProxyName".to_string(), proxy.to_string())];
    for (member, value) in sent {
        params.push(((*member).to_string(), (*value).to_string()));
    }
    call(router, "ModifyDBProxy", &params).await
}

/// The security group ids a `DBProxy` document reports, in the order it reports them.
///
/// `VpcSecurityGroupIds` and `VpcSubnetIds` are both `<member>` lists, so the ids are read
/// out of the one subtree that holds them rather than out of the whole document.
fn reported_security_groups(body: &str) -> Vec<String> {
    subtrees(body, "VpcSecurityGroupIds")
        .first()
        .map(|list| texts(list, "member"))
        .unwrap_or_default()
}

/// Assert every member of one row against one document (the modification's own answer, or
/// a later report of the proxy).
fn assert_settings(case: &str, source: &str, body: &str, row: &ModifyCase) {
    for (member, expected) in row.reported {
        let actual = text(body, member);
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected {source} to carry {member} {expected:?}, got {actual:?} \
             in {body}"
        );
    }
    let secret_arns = texts(body, "SecretArn");
    assert_eq!(
        secret_arns, row.auth_secret_arns,
        "case {case:?}: expected {source} to carry the authentication settings \
         {:?}, got {secret_arns:?} in {body}",
        row.auth_secret_arns
    );
    let groups = reported_security_groups(body);
    assert_eq!(
        groups, row.security_groups,
        "case {case:?}: expected {source} to carry the security groups {:?}, got {groups:?} \
         in {body}",
        row.security_groups
    );
}

#[tokio::test]
async fn a_proxy_modification_applies_what_it_states_and_keeps_the_rest() {
    let router = emulator();
    create_proxy(&router, "emu-modproxy-settings", "MYSQL").await;
    wait_for_settle();

    // The proxy starts with the settings its create gave it, which the first row is a
    // change TO: the two flags a create turns on only when asked, the documented default
    // idle timeout, the default authentication scheme, and no authentication settings at
    // all.
    let created = read_proxy(
        &router,
        "the proxy before any modification",
        "emu-modproxy-settings",
    )
    .await;
    for (member, expected) in [
        ("RequireTLS", "false".to_string()),
        ("DebugLogging", "false".to_string()),
        (
            "IdleClientTimeout",
            DEFAULT_IDLE_CLIENT_TIMEOUT.to_string(),
        ),
        ("DefaultAuthScheme", DEFAULT_AUTH_SCHEME_NONE.to_string()),
        ("Status", DB_PROXY_STATUS_AVAILABLE.to_string()),
    ] {
        let actual = text(&created, member);
        assert_eq!(
            actual, expected,
            "case \"before any modification\": expected {member} {expected:?}, got {actual:?} \
             in {created}"
        );
    }
    let created_role_arn = text(&created, "RoleArn");
    let created_endpoint = text(&created, "Endpoint");

    // The rows differ along exactly what the rule generalises over: WHICH members the
    // request states (one alone, several together, none at all), the KIND of each (a
    // boolean, an integer, an enum, a free-form ARN, a list of structures, a list of
    // strings) and, for the two lists, how MANY entries they carry — so the row after a
    // longer list proves the shorter one replaced it rather than being merged into it.
    //
    // The rows run in order against one proxy: a modification is a change to the proxy as
    // it stands, so each row starts from what the row before it left. Each row's
    // `reported` is the WHOLE of the proxy's settings, so every row also asserts the
    // members it did not state.
    // The rows the timeout has not been changed in yet state the documented default
    // literally, because a row is a table of static strings; this is what holds those rows
    // to the constant the service reads it from.
    assert_eq!(
        DEFAULT_IDLE_CLIENT_TIMEOUT.to_string(),
        "1800",
        "the rows below state the documented default idle timeout literally, so a change \
         to it must be carried into them"
    );
    let cases: &[ModifyCase] = &[
        ModifyCase {
            case: "one flag alone",
            sent: &[("DebugLogging", "true")],
            reported: &[
                ("RequireTLS", "false"),
                ("DebugLogging", "true"),
                ("IdleClientTimeout", "1800"),
                ("DefaultAuthScheme", DEFAULT_AUTH_SCHEME_NONE),
            ],
            auth_secret_arns: &[],
            security_groups: &[],
        },
        ModifyCase {
            case: "the other flag and the timeout together",
            sent: &[("RequireTLS", "true"), ("IdleClientTimeout", "60")],
            reported: &[
                ("RequireTLS", "true"),
                ("DebugLogging", "true"),
                ("IdleClientTimeout", "60"),
                ("DefaultAuthScheme", DEFAULT_AUTH_SCHEME_NONE),
            ],
            auth_secret_arns: &[],
            security_groups: &[],
        },
        ModifyCase {
            case: "a flag turned back off",
            sent: &[("DebugLogging", "false")],
            reported: &[
                ("RequireTLS", "true"),
                ("DebugLogging", "false"),
                ("IdleClientTimeout", "60"),
                ("DefaultAuthScheme", DEFAULT_AUTH_SCHEME_NONE),
            ],
            auth_secret_arns: &[],
            security_groups: &[],
        },
        ModifyCase {
            // `DefaultAuthScheme` takes `IAM_AUTH` or `NONE` (`smithy_query(query="shape",
            // shape="DefaultAuthScheme")`); `AuthScheme` takes `SECRETS` and `IAMAuth`
            // one of `DISABLED | REQUIRED | ENABLED`.
            case: "two authentication settings and the default scheme",
            sent: &[
                ("DefaultAuthScheme", "IAM_AUTH"),
                ("Auth.member.1.AuthScheme", "SECRETS"),
                ("Auth.member.1.SecretArn", FIRST_SECRET_ARN),
                ("Auth.member.1.UserName", "emu_app"),
                ("Auth.member.1.IAMAuth", "DISABLED"),
                ("Auth.member.1.Description", "the application user"),
                ("Auth.member.2.AuthScheme", "SECRETS"),
                ("Auth.member.2.SecretArn", SECOND_SECRET_ARN),
                ("Auth.member.2.UserName", "emu_reporting"),
                ("Auth.member.2.IAMAuth", "REQUIRED"),
            ],
            reported: &[
                ("RequireTLS", "true"),
                ("DebugLogging", "false"),
                ("IdleClientTimeout", "60"),
                ("DefaultAuthScheme", "IAM_AUTH"),
            ],
            auth_secret_arns: &[FIRST_SECRET_ARN, SECOND_SECRET_ARN],
            security_groups: &[],
        },
        ModifyCase {
            // "The new authentication settings for the DBProxy": the list REPLACES the two
            // the row above bound, so the proxy is left holding exactly this one.
            case: "a shorter new authentication list",
            sent: &[
                ("Auth.member.1.AuthScheme", "SECRETS"),
                ("Auth.member.1.SecretArn", SECOND_SECRET_ARN),
                ("Auth.member.1.UserName", "emu_reporting"),
            ],
            reported: &[
                ("RequireTLS", "true"),
                ("DebugLogging", "false"),
                ("IdleClientTimeout", "60"),
                ("DefaultAuthScheme", "IAM_AUTH"),
            ],
            auth_secret_arns: &[SECOND_SECRET_ARN],
            security_groups: &[],
        },
        ModifyCase {
            // "The new list of security groups for the DBProxy."
            case: "two security groups",
            sent: &[
                ("SecurityGroups.member.1", "sg-0a1b2c3d"),
                ("SecurityGroups.member.2", "sg-4e5f6a7b"),
            ],
            reported: &[
                ("RequireTLS", "true"),
                ("DebugLogging", "false"),
                ("IdleClientTimeout", "60"),
                ("DefaultAuthScheme", "IAM_AUTH"),
            ],
            auth_secret_arns: &[SECOND_SECRET_ARN],
            security_groups: &["sg-0a1b2c3d", "sg-4e5f6a7b"],
        },
        ModifyCase {
            case: "one security group in place of the two",
            sent: &[("SecurityGroups.member.1", "sg-8c9d0e1f")],
            reported: &[
                ("RequireTLS", "true"),
                ("DebugLogging", "false"),
                ("IdleClientTimeout", "60"),
                ("DefaultAuthScheme", "IAM_AUTH"),
            ],
            auth_secret_arns: &[SECOND_SECRET_ARN],
            security_groups: &["sg-8c9d0e1f"],
        },
        ModifyCase {
            case: "the role the proxy uses",
            sent: &[("RoleArn", OTHER_ROLE_ARN)],
            reported: &[
                ("RequireTLS", "true"),
                ("DebugLogging", "false"),
                ("IdleClientTimeout", "60"),
                ("DefaultAuthScheme", "IAM_AUTH"),
                ("RoleArn", OTHER_ROLE_ARN),
            ],
            auth_secret_arns: &[SECOND_SECRET_ARN],
            security_groups: &["sg-8c9d0e1f"],
        },
        ModifyCase {
            case: "a request that states no setting at all",
            sent: &[],
            reported: &[
                ("RequireTLS", "true"),
                ("DebugLogging", "false"),
                ("IdleClientTimeout", "60"),
                ("DefaultAuthScheme", "IAM_AUTH"),
                ("RoleArn", OTHER_ROLE_ARN),
            ],
            auth_secret_arns: &[SECOND_SECRET_ARN],
            security_groups: &["sg-8c9d0e1f"],
        },
    ];

    for row in cases {
        let answered = expect_ok(
            row.case,
            modify_proxy(&router, "emu-modproxy-settings", row.sent).await,
        );
        // The modification ANSWERS with the proxy it changed, and a later report of the
        // proxy agrees with that answer: the change was persisted, not echoed.
        let reported = read_proxy(&router, row.case, "emu-modproxy-settings").await;
        assert_settings(row.case, "the answer", &answered, row);
        assert_settings(row.case, "the report", &reported, row);
        // The change is accepted rather than instantaneous: the proxy reports that it is
        // applying it, from the answer onwards, and a further modification is taken while
        // it does — which is what the next row is.
        for (source, body) in [("the answer", &answered), ("the report", &reported)] {
            let actual = text(body, "Status");
            assert_eq!(
                actual, DB_PROXY_STATUS_MODIFYING,
                "case {:?}: expected {source} to report {DB_PROXY_STATUS_MODIFYING}, got \
                 {actual:?} in {body}",
                row.case
            );
        }
        // Nothing the request did not state moved: the name the proxy answers to, the
        // endpoint computed from it, and the role until the row that changes it.
        let actual = text(&reported, "DBProxyName");
        assert_eq!(
            actual, "emu-modproxy-settings",
            "case {:?}: a modification that renames nothing leaves the name alone, got \
             {actual:?} in {reported}",
            row.case
        );
        let actual = text(&reported, "Endpoint");
        assert_eq!(
            actual, created_endpoint,
            "case {:?}: the endpoint follows the proxy's name, which this row left alone, \
             got {actual:?} in {reported}",
            row.case
        );
    }

    // The role the create gave the proxy stood until the row that replaced it.
    assert_ne!(
        created_role_arn, OTHER_ROLE_ARN,
        "the create's role and the one a row moves the proxy to must differ for that row \
         to prove anything"
    );

    // The proxy comes back out of `modifying` on its own, with every change the rows made
    // still in force — the status settled, nothing was rolled back.
    wait_for_settle();
    let settled = read_proxy(
        &router,
        "the proxy once the change is in force",
        "emu-modproxy-settings",
    )
    .await;
    let actual = text(&settled, "Status");
    assert_eq!(
        actual, DB_PROXY_STATUS_AVAILABLE,
        "case \"once the change is in force\": expected {DB_PROXY_STATUS_AVAILABLE}, got \
         {actual:?} in {settled}"
    );
    assert_settings(
        "once the change is in force",
        "the report",
        &settled,
        &cases[cases.len() - 1],
    );
}

#[tokio::test]
async fn renaming_a_proxy_moves_it_and_everything_computed_from_its_name() {
    let router = emulator();
    create_proxy(&router, "emu-modproxy-from", "POSTGRESQL").await;
    create_proxy(&router, "emu-modproxy-taken", "MYSQL").await;
    wait_for_settle();

    let before = read_proxy(&router, "the proxy before the rename", "emu-modproxy-from").await;
    let minted_label = text(&before, "Endpoint")
        .split('.')
        .nth(1)
        .expect("the endpoint carries the server-assigned label as its second label")
        .to_string();

    // A rename onto a name the region already holds a proxy under would put two proxies
    // under one name.
    expect_error(
        "a rename onto a name the region already holds",
        modify_proxy(
            &router,
            "emu-modproxy-from",
            &[("NewDBProxyName", "emu-modproxy-taken")],
        )
        .await,
        400,
        "DBProxyAlreadyExistsFault",
    );
    let refused =
        read_proxy(&router, "the proxy after the refused rename", "emu-modproxy-from").await;
    assert_eq!(
        text(&refused, "Status"),
        DB_PROXY_STATUS_AVAILABLE,
        "a refused rename leaves the proxy exactly as it was: {refused}"
    );

    // Restating the name the proxy already has renames nothing.
    let unchanged = expect_ok(
        "a rename onto the name the proxy already has",
        modify_proxy(
            &router,
            "emu-modproxy-from",
            &[("NewDBProxyName", "emu-modproxy-from")],
        )
        .await,
    );
    assert_eq!(
        text(&unchanged, "DBProxyName"),
        "emu-modproxy-from",
        "a rename onto the proxy's own name leaves the name alone: {unchanged}"
    );

    // The rename moves the proxy: it answers to the new name, the old name resolves to
    // nothing, and everything computed from the name — the proxy's ARN, the endpoint a
    // client connects to, and the ARN of the one target group it has — follows it. The
    // server-assigned label inside the endpoint is NOT recomputed: it belongs to the
    // proxy, not to its name.
    let renamed = expect_ok(
        "the rename",
        modify_proxy(
            &router,
            "emu-modproxy-from",
            &[("NewDBProxyName", "emu-modproxy-to")],
        )
        .await,
    );
    for (member, expected) in [
        ("DBProxyName", "emu-modproxy-to".to_string()),
        (
            "DBProxyArn",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:db-proxy:emu-modproxy-to"),
        ),
        (
            "Endpoint",
            format!("emu-modproxy-to.{minted_label}.{REGION}.rds.amazonaws.com"),
        ),
    ] {
        let actual = text(&renamed, member);
        assert_eq!(
            actual, expected,
            "case \"the rename\": expected {member} {expected:?}, got {actual:?} in {renamed}"
        );
    }
    expect_error(
        "the name the proxy was renamed away from",
        call(
            &router,
            "DescribeDBProxies",
            &[("DBProxyName", "emu-modproxy-from")],
        )
        .await,
        404,
        "DBProxyNotFoundFault",
    );
    let moved = read_proxy(&router, "the proxy under its new name", "emu-modproxy-to").await;
    assert_eq!(
        text(&moved, "EngineFamily"),
        "POSTGRESQL",
        "the rename moved the proxy the create stored, not a new one: {moved}"
    );
    let group = expect_ok(
        "the renamed proxy's target group",
        call(
            &router,
            "DescribeDBProxyTargetGroups",
            &[("DBProxyName", "emu-modproxy-to")],
        )
        .await,
    );
    let actual = text(&group, "TargetGroupArn");
    assert_eq!(
        actual,
        format!("arn:aws:rds:{REGION}:{ACCOUNT}:target-group:emu-modproxy-to"),
        "the target group's ARN follows the proxy's name, got {actual:?} in {group}"
    );

    // The rename touched no other proxy.
    let bystander = read_proxy(&router, "the other proxy", "emu-modproxy-taken").await;
    assert_eq!(
        text(&bystander, "Status"),
        DB_PROXY_STATUS_AVAILABLE,
        "renaming one proxy must leave another alone: {bystander}"
    );
}

#[tokio::test]
async fn a_modification_the_proxy_cannot_take_is_refused_and_changes_nothing() {
    let router = emulator();

    // A proxy name that resolves to nothing is the proxy's own not-found fault.
    let unknown = expect_error(
        "an unknown proxy",
        modify_proxy(&router, "emu-modproxy-nope", &[("DebugLogging", "true")]).await,
        404,
        "DBProxyNotFoundFault",
    );
    assert!(
        text(&unknown, "Message").contains("emu-modproxy-nope"),
        "the not-found fault names the proxy the request asked for, got {unknown}"
    );

    // A proxy that has not finished coming up is in a state no modification acts in, and
    // the refusal names the state it is in.
    create_proxy(&router, "emu-modproxy-creating", "MYSQL").await;
    let creating = expect_error(
        "a proxy that is still coming up",
        modify_proxy(
            &router,
            "emu-modproxy-creating",
            &[("DebugLogging", "true")],
        )
        .await,
        400,
        "InvalidDBProxyStateFault",
    );
    assert!(
        creating.contains(DB_PROXY_STATUS_CREATING),
        "case \"a proxy that is still coming up\": expected the refusal to name the state \
         the proxy is in, got {creating}"
    );
    wait_for_settle();

    // The other half of the state rule: a proxy whose delete has been accepted is on its
    // way out, so there is nothing left for a modification to act on either.
    create_proxy(&router, "emu-modproxy-deleting", "MYSQL").await;
    wait_for_settle();
    expect_ok(
        "the delete of the proxy on its way out",
        call(
            &router,
            "DeleteDBProxy",
            &[("DBProxyName", "emu-modproxy-deleting")],
        )
        .await,
    );
    let deleting = expect_error(
        "a proxy whose delete has been accepted",
        modify_proxy(
            &router,
            "emu-modproxy-deleting",
            &[("DebugLogging", "true")],
        )
        .await,
        400,
        "InvalidDBProxyStateFault",
    );
    assert!(
        deleting.contains(DB_PROXY_STATUS_DELETING),
        "case \"a proxy whose delete has been accepted\": expected the refusal to name the \
         state the proxy is in, got {deleting}"
    );

    // A proxy of that name in another region is a proxy this one holds none of: RDS is
    // regional.
    expect_error(
        "a proxy of that name in another region",
        call_in(
            &router,
            "us-east-1",
            "ModifyDBProxy",
            &[
                ("DBProxyName", "emu-modproxy-creating"),
                ("DebugLogging", "true"),
            ],
        )
        .await,
        404,
        "DBProxyNotFoundFault",
    );

    // A value outside what a member accepts is refused with the proxy left as it was: a
    // security group id that is not of the form that identifier takes, and free-form text
    // longer than this service stores.
    let too_long = "a".repeat(MAX_FREE_FORM_LEN + 1);
    let refusals: Vec<(&str, Vec<(&str, String)>)> = vec![
        (
            "a security group id that is not of the published form",
            vec![("SecurityGroups.member.1", "-sg-not-an-id".to_string())],
        ),
        (
            "one bad security group id among good ones",
            vec![
                ("SecurityGroups.member.1", "sg-0a1b2c3d".to_string()),
                ("SecurityGroups.member.2", "sg bad".to_string()),
            ],
        ),
        (
            "an authentication description longer than this service stores",
            vec![
                ("Auth.member.1.AuthScheme", "SECRETS".to_string()),
                ("Auth.member.1.SecretArn", FIRST_SECRET_ARN.to_string()),
                ("Auth.member.1.Description", too_long.clone()),
            ],
        ),
    ];
    for (case, params) in &refusals {
        let mut request: Vec<(&str, String)> =
            vec![("DBProxyName", "emu-modproxy-creating".to_string())];
        request.extend(params.iter().cloned());
        expect_error(
            case,
            call(&router, "ModifyDBProxy", &request).await,
            400,
            "InvalidParameterValue",
        );
    }

    // `UserAuthConfigList` publishes "len 0..200": the highest count the member accepts is
    // served and stored, and one entry past it is a value the member does not accept. The
    // bound is exercised on a proxy of its own, because the accepted call binds settings.
    create_proxy(&router, "emu-modproxy-authbound", "MYSQL").await;
    wait_for_settle();
    let auth_entries = |count: usize| -> Vec<(String, String)> {
        let mut params = vec![(
            "DBProxyName".to_string(),
            "emu-modproxy-authbound".to_string(),
        )];
        for entry in 1..=count {
            params.push((
                format!("Auth.member.{entry}.AuthScheme"),
                "SECRETS".to_string(),
            ));
            params.push((
                format!("Auth.member.{entry}.SecretArn"),
                format!("{FIRST_SECRET_ARN}-{entry}"),
            ));
        }
        params
    };
    let served = expect_ok(
        "the highest authentication-settings count the member accepts",
        call(&router, "ModifyDBProxy", &auth_entries(MAX_USER_AUTH_CONFIGS)).await,
    );
    let bound_count = texts(&served, "SecretArn").len();
    assert_eq!(
        bound_count, MAX_USER_AUTH_CONFIGS,
        "case \"the highest count the member accepts\": expected {MAX_USER_AUTH_CONFIGS} \
         authentication settings, got {bound_count} in {served}"
    );
    expect_error(
        "one authentication setting past the published count",
        call(
            &router,
            "ModifyDBProxy",
            &auth_entries(MAX_USER_AUTH_CONFIGS + 1),
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    let bounded = read_proxy(
        &router,
        "the proxy after the count refusal",
        "emu-modproxy-authbound",
    )
    .await;
    let kept_count = texts(&bounded, "SecretArn").len();
    assert_eq!(
        kept_count, MAX_USER_AUTH_CONFIGS,
        "case \"after the count refusal\": the refused call left the settings the accepted \
         one stored, got {kept_count} in {bounded}"
    );

    // Every refusal left the proxy exactly as its create stored it: still available, still
    // with the flags off and no authentication settings or security groups of its own.
    let after = read_proxy(&router, "the proxy after the refusals", "emu-modproxy-creating").await;
    for (member, expected) in [
        ("Status", DB_PROXY_STATUS_AVAILABLE.to_string()),
        ("DebugLogging", "false".to_string()),
        ("RequireTLS", "false".to_string()),
        (
            "IdleClientTimeout",
            DEFAULT_IDLE_CLIENT_TIMEOUT.to_string(),
        ),
    ] {
        let actual = text(&after, member);
        assert_eq!(
            actual, expected,
            "case \"after the refusals\": expected {member} {expected:?}, got {actual:?} in \
             {after}"
        );
    }
    assert!(
        texts(&after, "SecretArn").is_empty(),
        "case \"after the refusals\": a refused modification binds no authentication \
         settings, got {after}"
    );
    assert!(
        reported_security_groups(&after).is_empty(),
        "case \"after the refusals\": a refused modification stores no security group, got \
         {after}"
    );
}
