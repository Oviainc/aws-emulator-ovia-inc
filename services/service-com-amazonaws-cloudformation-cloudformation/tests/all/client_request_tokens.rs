// SPDX-License-Identifier: CC0-1.0

//! The `ClientRequestToken` a mutating stack operation runs under.
//!
//! RULE: the token a mutating stack operation carries names THAT operation on THAT stack —
//! `CreateStackInput$ClientRequestToken`: "a unique identifier for this `CreateStack`
//! request. Specify this token if you plan to retry requests so that CloudFormation knows
//! that you're not attempting to create a stack with the same name", and
//! `TokenAlreadyExistsException`: "A client request token already exists" (`errorKind`
//! client, `httpError` 400). So a create repeating a live stack's name under the token that
//! created it is that retry and answers the SAME stack id instead of
//! `AlreadyExistsException`, while any other mutating operation presenting a token an
//! operation on that stack already claimed is refused HTTP 400 with the wire code
//! `TokenAlreadyExistsException` and changes nothing.
//!
//! The rows differ along what the rule generalises over: the token's value, its length at
//! each published bound of the modelled `ClientRequestToken` (`@length(1..128)`,
//! `@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`), its leading character class, and the console's
//! own documented token format. The operation the token is presented to varies too —
//! `CreateStack`, `UpdateStack`, `DeleteStack` and `ExecuteChangeSet` are the implemented
//! mutating operations whose model carries `TokenAlreadyExistsException` — and so does the
//! stack it is presented against, since a token is scoped to ONE stack.
//!
//! The issue's own case — a create under a token, then an `UpdateStack` reusing it — is one
//! row's first two steps.
//!
//! Everything goes over the awsQuery wire of the router `harness.rs` builds, so what is
//! asserted is what a client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, get_template, member, router_with_engine, xml_text,
};

/// The wire code a request presenting a token already claimed for that stack is refused
/// under (`TokenAlreadyExistsException`'s `awsQueryError` code, `httpResponseCode` 400).
const ALREADY_USED: &str = "TokenAlreadyExistsException";

/// The wire code a create of a name that is already taken is refused under.
const ALREADY_EXISTS: &str = "AlreadyExistsException";

/// The template a row's stack is created from.
const CREATED: &str =
    r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// A template that differs from [`CREATED`], so an update of it really changes the stack
/// ("No updates are to be performed." is what an update changing nothing is refused with).
const UPDATED: &str = r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"Second":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// A third template, for a row's second update.
const UPDATED_AGAIN: &str = r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"Third":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// `UpdateStack` with a template and, when the case names one, a `ClientRequestToken`.
async fn update_stack(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
    token: Option<&str>,
) -> (u16, String) {
    let mut members = vec![
        member("StackName", name),
        member("TemplateBody", template),
    ];
    if let Some(token) = token {
        members.push(member("ClientRequestToken", token));
    }
    call(router, "UpdateStack", &members).await
}

/// `DeleteStack`, with the case's `ClientRequestToken` when it names one.
async fn delete_stack(
    router: &MultiServiceRouter,
    name: &str,
    token: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("StackName", name)];
    if let Some(token) = token {
        members.push(member("ClientRequestToken", token));
    }
    call(router, "DeleteStack", &members).await
}

/// `CreateStack` carrying a `ClientRequestToken`.
async fn create_with_token(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
    token: &str,
) -> (u16, String) {
    create_stack(
        router,
        name,
        template,
        &[member("ClientRequestToken", token)],
    )
    .await
}

/// A token distinct from `token` and still inside the modelled maximum length: the suffix
/// replaces the end of the token rather than extending it past 128 characters, so a row at
/// the length bound derives a sibling token the model still admits.
fn sibling_token(token: &str, suffix: &str) -> String {
    let keep = token.len().min(128usize.saturating_sub(suffix.len()));
    let keep = (0..=keep).rev().find(|at| token.is_char_boundary(*at)).unwrap_or(0);
    format!("{}{suffix}", &token[..keep])
}

/// Assert a response is the duplicate-token refusal: HTTP 400, the modelled wire code, a
/// message naming the token the request reused, and the envelope's own request id.
fn assert_already_used(case: &str, token: &str, status: u16, body: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: a reused client request token must be refused, got {status}: {body}"
    );
    let code = xml_text(body, "Code");
    assert_eq!(
        code.as_deref(),
        Some(ALREADY_USED),
        "case {case:?}: expected the wire code {ALREADY_USED}, got {code:?}: {body}"
    );
    let message = xml_text(body, "Message").unwrap_or_default();
    assert!(
        message.contains(token),
        "case {case:?}: the refusal must name the token the request reused ({token:?}), \
         got {message:?}"
    );
    assert!(
        xml_text(body, "RequestId").is_some(),
        "case {case:?}: the error envelope must carry a RequestId, got {body}"
    );
}

/// The `StackId` a response reports, or a panic naming the case.
fn stack_id(case: &str, body: &str) -> String {
    xml_text(body, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a StackId, got {body}"))
}

/// One row of the table: a token value the modelled `ClientRequestToken` admits.
struct Case {
    case: &'static str,
    /// The stack the row drives, so rows cannot collide.
    stack_name: String,
    /// The token the row's create claims.
    token: String,
}

/// The table. The values span the modelled constraints: `@length(1..128)` at both published
/// bounds, the pattern's two leading classes (a letter and a digit), the hyphen the pattern
/// admits inside, and the token format `CreateStackInput$ClientRequestToken`'s own
/// documentation gives for console operations.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a one-character token (the modelled minimum length)",
            stack_name: "tok-min".to_string(),
            token: "a".to_string(),
        },
        Case {
            case: "a digit-leading token",
            stack_name: "tok-digit".to_string(),
            token: "0".to_string(),
        },
        Case {
            case: "a hyphenated mixed-case token",
            stack_name: "tok-hyphen".to_string(),
            token: "Probe-Token-42".to_string(),
        },
        Case {
            // The documented console form: "Console-CreateStack-<uuid>".
            case: "the console's own documented token format",
            stack_name: "tok-console".to_string(),
            token: "Console-CreateStack-7f59c3cf-00d2-40c7-b2ff-e75db0987002".to_string(),
        },
        Case {
            case: "a 128-character token (the modelled maximum length)",
            stack_name: "tok-max".to_string(),
            token: format!("t{}", "a".repeat(127)),
        },
    ]
}

/// The rule over the table, end to end: the create's token makes a repeat of that create a
/// retry, and makes every LATER operation presenting it a refusal that changes nothing.
#[tokio::test]
async fn a_token_names_one_operation_on_one_stack() {
    let (router, engine) = router_with_engine();

    for case in cases() {
        let name = case.stack_name.as_str();
        let token = case.token.as_str();
        let label = case.case;

        // The create claims the token.
        let (status, created) = create_with_token(&router, name, CREATED, token).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a create under a fresh token must be accepted, got {status}: \
             {created}"
        );
        let created_id = stack_id(label, &created);

        // "Specify this token if you plan to retry requests": the SAME create again is
        // that retry, and it answers the SAME stack rather than refusing the name.
        let (status, retried) = create_with_token(&router, name, CREATED, token).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a create retried under its own token must be accepted, got \
             {status}: {retried}"
        );
        assert_eq!(
            stack_id(label, &retried),
            created_id,
            "case {label:?}: the retry must answer the stack the first create made, got \
             {retried}"
        );

        // A create of the same name under ANOTHER token is not a retry of anything.
        let (status, refused) =
            create_with_token(&router, name, CREATED, &sibling_token(token, "-other")).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a second create under another token must be refused, got \
             {status}: {refused}"
        );
        let code = xml_text(&refused, "Code");
        assert_eq!(
            code.as_deref(),
            Some(ALREADY_EXISTS),
            "case {label:?}: expected the taken-name code {ALREADY_EXISTS}, got {code:?}: \
             {refused}"
        );

        // A DIFFERENT operation presenting the create's token is the duplicate submission
        // the modelled error refuses — and it is weighed before the stack's own status,
        // which is still the create's.
        let (status, refused) = update_stack(&router, name, UPDATED, Some(token)).await;
        assert_already_used(label, token, status, &refused);

        settle_the_store(&router, &engine).await;
        let (status, kept) = get_template(&router, name, None).await;
        assert_eq!(status, 200, "case {label:?}: GetTemplate answered {status}: {kept}");
        let stored = xml_text(&kept, "TemplateBody").unwrap_or_default();
        assert!(
            !stored.contains("Second"),
            "case {label:?}: a refused update must leave the stack's template alone, got \
             {stored:?}"
        );

        // The same update under a token new to this stack is accepted.
        let fresh = sibling_token(token, "-update");
        let (status, updated) = update_stack(&router, name, UPDATED, Some(&fresh)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an update under a fresh token must be accepted, got {status}: \
             {updated}"
        );
        settle_the_store(&router, &engine).await;

        // And that token is now claimed in its turn.
        let (status, refused) = update_stack(&router, name, UPDATED_AGAIN, Some(&fresh)).await;
        assert_already_used(label, &fresh, status, &refused);

        // A CREATE presenting a token one of the stack's OTHER operations claimed is not a
        // retry of anything either: it is the same duplicate submission, not a name clash.
        let (status, refused) = create_with_token(&router, name, CREATED, &fresh).await;
        assert_already_used(label, &fresh, status, &refused);

        // `DeleteStack` is held to the same rule, and a refused delete keeps the stack.
        let (status, refused) = delete_stack(&router, name, Some(token)).await;
        assert_already_used(label, token, status, &refused);
        let (status, described) = describe_stack(&router, name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a refused delete must leave the stack readable, got {status}: \
             {described}"
        );

        let deleting = sibling_token(token, "-delete");
        let (status, deleted) = delete_stack(&router, name, Some(&deleting)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a delete under a fresh token must be accepted, got {status}: \
             {deleted}"
        );

        // While that deletion is IN FLIGHT: its own token is the retry the member
        // documents, and gets the no-op success a delete already under way answers; a token
        // one of the stack's other operations claimed is still the duplicate submission.
        let (status, retried) = delete_stack(&router, name, Some(&deleting)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the deletion retried under its own token must be accepted, got \
             {status}: {retried}"
        );
        let (status, refused) = delete_stack(&router, name, Some(&fresh)).await;
        assert_already_used(label, &fresh, status, &refused);
    }
}

/// A token is scoped to ONE stack ("already been used for this stack"): the same token
/// claimed by one stack's create is still free for another stack's.
#[tokio::test]
async fn a_token_claimed_on_one_stack_is_free_on_another() {
    let (router, engine) = router_with_engine();
    let token = "shared-across-stacks";

    let (status, first) = create_with_token(&router, "scope-first", CREATED, token).await;
    assert_eq!(
        status, 200,
        "the first stack's create must be accepted, got {status}: {first}"
    );
    let (status, second) = create_with_token(&router, "scope-second", CREATED, token).await;
    assert_eq!(
        status, 200,
        "case \"the same token on another stack\": the create must be accepted, got \
         {status}: {second}"
    );
    assert_ne!(
        stack_id("the second stack", &second),
        stack_id("the first stack", &first),
        "the two stacks must be two stacks, got {second}"
    );

    settle_the_store(&router, &engine).await;

    // Each stack refuses the token its OWN create claimed.
    for name in ["scope-first", "scope-second"] {
        let (status, refused) = update_stack(&router, name, UPDATED, Some(token)).await;
        assert_already_used(name, token, status, &refused);
    }
}

/// `ExecuteChangeSet` is held to the rule too: the operation it opens on the stack cannot
/// run under a token the stack's create already claimed.
#[tokio::test]
async fn execute_change_set_is_held_to_the_token_rule() {
    let (router, engine) = router_with_engine();
    let token = "change-set-stack-token";

    let (status, created) = create_with_token(&router, "tok-change-set", CREATED, token).await;
    assert_eq!(
        status, 200,
        "the stack's create must be accepted, got {status}: {created}"
    );
    settle_the_store(&router, &engine).await;

    let (status, planned) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", "tok-change-set"),
            member("ChangeSetName", "tok-plan"),
            member("TemplateBody", UPDATED),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the change set must be created, got {status}: {planned}"
    );
    settle_the_store(&router, &engine).await;

    let execute = |token: Option<&'static str>| {
        let mut members = vec![
            member("StackName", "tok-change-set"),
            member("ChangeSetName", "tok-plan"),
        ];
        if let Some(token) = token {
            members.push(member("ClientRequestToken", token));
        }
        members
    };

    let (status, refused) = call(&router, "ExecuteChangeSet", &execute(Some(token))).await;
    assert_already_used("an execution under the create's token", token, status, &refused);

    let (status, executed) = call(
        &router,
        "ExecuteChangeSet",
        &execute(Some("change-set-execution-token")),
    )
    .await;
    assert_eq!(
        status, 200,
        "an execution under a fresh token must be accepted, got {status}: {executed}"
    );

    // The execution CLAIMED its token in its turn: a later operation presenting it is the
    // same duplicate submission.
    settle_the_store(&router, &engine).await;
    let (status, refused) = update_stack(
        &router,
        "tok-change-set",
        UPDATED_AGAIN,
        Some("change-set-execution-token"),
    )
    .await;
    assert_already_used(
        "an update under the execution's token",
        "change-set-execution-token",
        status,
        &refused,
    );
}
