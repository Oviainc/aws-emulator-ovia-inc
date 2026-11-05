// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE field split every ARN this service reads is read
//! through.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: an ARN is six colon-separated fields,
//! `arn:<partition>:<service>:<region>:<account>:<resource>`, whose first field is
//! literally `arn` and whose resource field keeps every colon it carries — so reading
//! ANY field out of a value takes all six fields being present, and the readers of this
//! service differ only in which of the fields they then require. Two rules read fields:
//!
//! * the cross-account rule a write applies reads the ACCOUNT (the fifth field), so it
//!   accepts a `sourceRoleArn` only when the value presents six fields and that field is
//!   non-empty and names another account — `CreateInvestigationGroup` and
//!   `UpdateInvestigationGroup`;
//! * identifier resolution reads the SERVICE namespace (the third field) and the
//!   RESOURCE (the sixth), so it addresses a group only when the value is that group's
//!   ARN field for field — `GetInvestigationGroup` through its modelled
//!   `{identifier}`, and the tagging operations through their unconstrained
//!   `resourceArn` (`smithy_query(query="operation",operation="ListTagsForResource")`:
//!   `resourceArn` targets `String`, so a value of any shape reaches the resolver there).
//!
//! The rows below are FORMS: each reassembles the trailing fields of a complete ARN in
//! one way the grammar admits or refuses, and every row is driven through an operation
//! of each reader, so the two can neither disagree about what an ARN is nor about which
//! field they read out of it.
//!
//! An account holds one investigation group per Region, so each row drives its create in
//! a Region of its own and its reads against a Region that already holds a group.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// An account OTHER than the investigation account (`engine::EMULATOR_ACCOUNT_ID`, the
/// account every minted ARN carries), which a cross-account source role may name — a
/// source role naming the investigation account itself is a refusal of its own, pinned
/// by `tests/all/cross_account_configuration_names_an_account.rs`.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`): the account an
/// investigation group belongs to, which the cross-account refusal names.
const ACCOUNT: &str = "000000000000";

/// A role ARN for the group itself, satisfying the modelled `RoleArn`
/// (`@length 20..2048`, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The modelled error shape a refused write reports
/// (`smithy_query(query="operation",operation="CreateInvestigationGroup")`:
/// `ValidationException`, `@httpError 400`).
const VALIDATION: &str = "ValidationException";

/// The modelled error shape an identifier that addresses no group reports
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`: `@httpError 404`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code the tagging frontend reports an unresolvable `resourceArn` with — see
/// `state::TAGGING_RESOURCE_NOT_FOUND`.
const TAGGING_NOT_FOUND: &str = "NotFoundException";

/// The member a refused cross-account entry is sent on.
const SOURCE_ROLE_MEMBER: &str = "sourceRoleArn";

/// The shortest `sourceRoleArn` the model admits, from the member's own `@length`
/// (`smithy_query(query="shape",shape="CrossAccountConfiguration")`: it targets
/// `RoleArn`, `@length` 20..2048). A shorter value is refused by the constraint walk at
/// the request boundary, before the handlers' own cross-account rule sees it.
const MIN_SOURCE_ROLE_ARN_LENGTH: usize = 20;

/// The `@pattern` the model declares for the member, as this service reports it
/// (unanchored): `smithy_query(query="shape",shape="RoleArn")` declares `^arn:.*$`.
const SOURCE_ROLE_ARN_PATTERN: &str = "arn:.*";

/// The sentence a source role that names no account other than the investigation
/// account is refused with: fixed text around the investigation account and the ARN the
/// request sent.
fn source_role_sentence(source_role_arn: &str) -> String {
    format!(
        "Source role ARN cannot belong to the investigation account {ACCOUNT}. \
         Found ARN: {source_role_arn}"
    )
}

/// The whole message the write refusing `source_role_arn` must answer: the boundary
/// walk's clauses when the value breaks the member's own modelled constraints, and the
/// service's own sentence for every other value the cross-account rule refuses.
fn expected_refusal(source_role_arn: &str) -> String {
    let path = format!("crossAccountConfigurations.1.member.{SOURCE_ROLE_MEMBER}");
    let mut clauses: Vec<String> = Vec::new();
    if source_role_arn.len() < MIN_SOURCE_ROLE_ARN_LENGTH {
        clauses.push(format!(
            "Value at '{path}' failed to satisfy constraint: Member must have length \
             greater than or equal to {MIN_SOURCE_ROLE_ARN_LENGTH}"
        ));
    }
    if !source_role_arn.starts_with("arn:") {
        clauses.push(format!(
            "Value at '{path}' failed to satisfy constraint: Member must satisfy regular \
             expression pattern: {SOURCE_ROLE_ARN_PATTERN}"
        ));
    }
    match clauses.as_slice() {
        [] => source_role_sentence(source_role_arn),
        [clause] => format!("1 validation error detected: {clause}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The tag the target group is created with, so a tag read that resolved the group is
/// distinguishable from one that answered an empty map.
const TAG_KEY: &str = "Team";
const TAG_VALUE: &str = "ops";

/// Real AWS Regions the rows sign for, in order; past the list the names are synthetic
/// ones inside what a SigV4 credential scope admits (`[a-zA-Z0-9-]`).
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "us-east-2",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "ap-southeast-1",
    "ap-northeast-1",
    "sa-east-1",
    "ca-central-1",
    "ap-south-1",
    "eu-north-1",
    "me-south-1",
    "af-south-1",
    "ap-east-1",
    "ap-southeast-4",
];

/// The Region slot `n` signs for; each row needs two of its own.
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:04}"), |name| (*name).to_string())
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on and the
/// handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The message a failed call reported.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode everything a URI component may not carry literally — what an SDK does
/// for a non-greedy `@httpLabel` path segment.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Create the row's target group in `region` — tagged, so a tag read that resolved it is
/// visible — and answer its ARN.
async fn create_target(router: &MultiServiceRouter, region: &str) -> String {
    let (status, created) = call_in(
        router,
        region,
        "POST",
        "/investigationGroups",
        Some(&json!({
            "name": "arn-field-split-target",
            "roleArn": ROLE_ARN,
            "tags": { TAG_KEY: TAG_VALUE },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "creating the target in {region}: expected 201, got {status} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("creating the target in {region}: no arn in {created:?}"))
        .to_string()
}

/// The six fields of a complete ARN this test renders its forms from.
struct Fields<'a> {
    partition: &'a str,
    service: &'a str,
    region: &'a str,
    account: &'a str,
    resource: &'a str,
}

/// The trailing five fields of `arn`, which every row reassembles in its own way.
fn fields_of(arn: &str) -> Fields<'_> {
    let parts: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        parts.len(),
        6,
        "a minted ARN carries six colon-separated fields, got {arn:?}"
    );
    assert_eq!(
        parts[0], "arn",
        "a minted ARN opens with the literal arn field, got {arn:?}"
    );
    Fields {
        partition: parts[1],
        service: parts[2],
        region: parts[3],
        account: parts[4],
        resource: parts[5],
    }
}

/// One form a value can arrive in, and what each reader may take out of it.
struct Form {
    case: &'static str,
    /// Reassembles a value from the fields of a complete ARN.
    render: fn(&Fields<'_>) -> String,
    /// Whether the rendered value presents six colon-separated fields opening with the
    /// literal `arn` and carrying a non-empty FIFTH field: what reading an account out
    /// of it takes. Every row renders the account field as another account, so a form
    /// that names one is a form the cross-account rule accepts.
    names_an_account: bool,
    /// Whether the rendered value is the group's own ARN, field for field: what
    /// addressing the stored group takes.
    addresses_the_group: bool,
    /// Whether the rendered value is inside the modelled `InvestigationGroupIdentifier`
    /// pattern — `^(?:[\-_A-Za-z0-9]{1,512}|arn:(aws|aws-us-gov|aws-cn|aws-iso|aws-iso-b)
    /// :aiops:[a-zA-Z0-9-]*:[0-9]{12}:investigation-group\/[A-Za-z0-9]{16})$`
    /// (`smithy_query(query="shape",shape="InvestigationGroupIdentifier")`) — and so
    /// whether it reaches the resolver through `GetInvestigationGroup`'s `{identifier}`
    /// at all, rather than being refused by the constraint walk. The tagging
    /// operations' `resourceArn` carries no pattern, so every row reaches the resolver
    /// there.
    modelled_identifier: bool,
}

/// The table. The rule generalises over the FORM a value arrives in, so the rows differ
/// along which field is varied (the leading `arn` field, the partition, the service
/// namespace, the region, the account, the resource), along how many fields the value
/// carries at all, along which fields are present but EMPTY, and along whether the
/// resource field itself carries colons.
fn forms() -> Vec<Form> {
    vec![
        Form {
            case: "a complete ARN",
            render: |f| {
                format!(
                    "arn:{}:{}:{}:{}:{}",
                    f.partition, f.service, f.region, f.account, f.resource
                )
            },
            names_an_account: true,
            addresses_the_group: true,
            modelled_identifier: true,
        },
        Form {
            case: "a resource field that itself carries colons",
            render: |f| {
                format!(
                    "arn:{}:{}:{}:{}:{}:scoped",
                    f.partition, f.service, f.region, f.account, f.resource
                )
            },
            names_an_account: true,
            addresses_the_group: false,
            modelled_identifier: false,
        },
        Form {
            case: "an empty resource field",
            render: |f| {
                format!(
                    "arn:{}:{}:{}:{}:",
                    f.partition, f.service, f.region, f.account
                )
            },
            names_an_account: true,
            addresses_the_group: false,
            modelled_identifier: false,
        },
        Form {
            case: "an empty region field",
            render: |f| {
                format!(
                    "arn:{}:{}::{}:{}",
                    f.partition, f.service, f.account, f.resource
                )
            },
            names_an_account: true,
            addresses_the_group: false,
            modelled_identifier: true,
        },
        Form {
            case: "an empty account field",
            render: |f| {
                format!(
                    "arn:{}:{}:{}::{}",
                    f.partition, f.service, f.region, f.resource
                )
            },
            names_an_account: false,
            addresses_the_group: false,
            modelled_identifier: false,
        },
        Form {
            case: "another partition in the second field",
            render: |f| {
                format!(
                    "arn:aws-cn:{}:{}:{}:{}",
                    f.service, f.region, f.account, f.resource
                )
            },
            names_an_account: true,
            addresses_the_group: false,
            modelled_identifier: true,
        },
        Form {
            case: "another service namespace in the third field",
            render: |f| {
                format!(
                    "arn:{}:logs:{}:{}:{}",
                    f.partition, f.region, f.account, f.resource
                )
            },
            names_an_account: true,
            addresses_the_group: false,
            modelled_identifier: false,
        },
        Form {
            case: "a value that stops at the account field",
            render: |f| {
                format!(
                    "arn:{}:{}:{}:{}",
                    f.partition, f.service, f.region, f.account
                )
            },
            names_an_account: false,
            addresses_the_group: false,
            modelled_identifier: false,
        },
        Form {
            case: "a value with nothing but a partition and a service",
            render: |f| format!("arn:{}:{}", f.partition, f.service),
            names_an_account: false,
            addresses_the_group: false,
            modelled_identifier: false,
        },
        Form {
            case: "six fields whose first is not the literal arn field",
            render: |f| {
                format!(
                    "xarn:{}:{}:{}:{}:{}",
                    f.partition, f.service, f.region, f.account, f.resource
                )
            },
            names_an_account: false,
            addresses_the_group: false,
            modelled_identifier: false,
        },
        Form {
            case: "the fields of an ARN with no leading arn field at all",
            render: |f| {
                format!(
                    "{}:{}:{}:{}:{}",
                    f.partition, f.service, f.region, f.account, f.resource
                )
            },
            names_an_account: false,
            addresses_the_group: false,
            modelled_identifier: false,
        },
    ]
}

/// One cross-account list naming `role` as its only source role.
fn entries(role: &str) -> Value {
    json!([{ SOURCE_ROLE_MEMBER: role }])
}

#[tokio::test]
async fn both_arn_readers_take_their_field_from_one_split() {
    let router = emulator();

    for (index, form) in forms().iter().enumerate() {
        let case = form.case;
        // One Region with an empty store for the create, and one already holding the
        // group every read of this row addresses.
        let create_region = region(2 * index);
        let home = region(2 * index + 1);
        let arn = create_target(&router, &home).await;

        // The row's form, rendered twice: once over the fields of an IAM role in another
        // account — what the cross-account rule reads — and once over the fields of the
        // stored group's own ARN, which is what identifier resolution reads.
        let role = (form.render)(&Fields {
            partition: "aws",
            service: "iam",
            region: "",
            account: FOREIGN_ACCOUNT,
            resource: "role/AIOpsCrossAccountRole",
        });
        let identifier = (form.render)(&fields_of(&arn));
        assert_eq!(
            identifier == arn,
            form.addresses_the_group,
            "case {case:?}: a form addresses the group exactly when it reproduces the \
             stored ARN {arn:?}, got {identifier:?}"
        );

        // --- the ACCOUNT field, through the two writes that read it ---
        let (create_status, created) = call_in(
            &router,
            &create_region,
            "POST",
            "/investigationGroups",
            Some(&json!({
                "name": format!("arn-field-split-source-{index}"),
                "roleArn": ROLE_ARN,
                "crossAccountConfigurations": entries(&role),
            })),
        )
        .await;
        let (update_status, updated) = call_in(
            &router,
            &home,
            "PATCH",
            &format!("/investigationGroups/{}", encode(&arn)),
            Some(&json!({ "crossAccountConfigurations": entries(&role) })),
        )
        .await;

        if form.names_an_account {
            assert_eq!(
                create_status, 201,
                "case {case:?}: {role:?} names an account, so the create must be \
                 accepted, got {create_status} / {created:?}"
            );
            assert_eq!(
                update_status, 200,
                "case {case:?}: {role:?} names an account, so the update must be \
                 accepted, got {update_status} / {updated:?}"
            );
            // Accepted means STORED: the list the update carried is what a later read
            // reports, source role for source role.
            let (status, after) = call_in(
                &router,
                &home,
                "GET",
                &format!("/investigationGroups/{}", encode(&arn)),
                None,
            )
            .await;
            assert_eq!(
                (status, after.get("crossAccountConfigurations")),
                (200, Some(&entries(&role))),
                "case {case:?}: expected the read-back to report {:?}, got {status} / \
                 {after:?}",
                entries(&role)
            );
        } else {
            for (operation, status, body) in [
                ("CreateInvestigationGroup", create_status, &created),
                ("UpdateInvestigationGroup", update_status, &updated),
            ] {
                assert_eq!(
                    status, 400,
                    "case {case:?}: {role:?} names no account, so {operation} must refuse \
                     it with 400, got {status} / {body:?}"
                );
                assert_eq!(
                    error_code(body),
                    VALIDATION,
                    "case {case:?}: expected {operation} to report {VALIDATION}, got \
                     {body:?}"
                );
                let expected = expected_refusal(&role);
                assert_eq!(
                    message(body),
                    expected,
                    "case {case:?}: expected {operation} to answer {expected:?}, got {:?}",
                    message(body)
                );
            }
        }

        // --- the SERVICE and RESOURCE fields, through the readers that resolve a group ---
        // The tagging frontend's `resourceArn` carries no pattern, so every form reaches
        // the resolver there.
        let (status, tags) = call_in(
            &router,
            &home,
            "GET",
            &format!("/tags/{}", encode(&identifier)),
            None,
        )
        .await;
        if form.addresses_the_group {
            assert_eq!(
                (status, tags.get("tags")),
                (200, Some(&json!({ TAG_KEY: TAG_VALUE }))),
                "case {case:?}: {identifier:?} addresses the group, so its tags must be \
                 reported, got {status} / {tags:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&tags).as_str()),
                (404, TAGGING_NOT_FOUND),
                "case {case:?}: {identifier:?} addresses no group, so a tag read must \
                 report {TAGGING_NOT_FOUND}, got {status} / {tags:?}"
            );
        }

        // And through `GetInvestigationGroup`'s own modelled `{identifier}`, which the
        // constraint walk refuses before the resolver for a form outside its pattern.
        let (status, answer) = call_in(
            &router,
            &home,
            "GET",
            &format!("/investigationGroups/{}", encode(&identifier)),
            None,
        )
        .await;
        if !form.modelled_identifier {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, VALIDATION),
                "case {case:?}: {identifier:?} is outside the modelled identifier \
                 pattern, so the walk must refuse it, got {status} / {answer:?}"
            );
        } else if form.addresses_the_group {
            assert_eq!(
                (status, answer.get("arn").and_then(Value::as_str)),
                (200, Some(arn.as_str())),
                "case {case:?}: {identifier:?} is the group's ARN, so the group must be \
                 reported, got {status} / {answer:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (404, RESOURCE_NOT_FOUND),
                "case {case:?}: {identifier:?} addresses no group, so the read must \
                 report {RESOURCE_NOT_FOUND}, got {status} / {answer:?}"
            );
        }

        // Nothing above reached another Region's store: this Region still holds exactly
        // the one group the row created.
        let (status, listed) = call_in(&router, &home, "GET", "/investigationGroups", None).await;
        let arns: Vec<&str> = listed
            .get("investigationGroups")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no investigationGroups in {listed:?}"))
            .iter()
            .filter_map(|group| group.get("arn").and_then(Value::as_str))
            .collect();
        assert_eq!(
            (status, arns.as_slice()),
            (200, [arn.as_str()].as_slice()),
            "case {case:?}: the Region must still hold exactly its own group, got \
             {status} / {listed:?}"
        );
    }
}

/// The account a cross-account source role names is read out of the SAME fields a group
/// identifier is read out of, so the two readers agree about the account field of a value
/// whose resource portion carries colons: the resource is the sixth field and everything
/// after it, never split further.
///
/// Rows differ along what follows the account field — a path, colons, both, and nothing —
/// over the one form both readers accept.
#[tokio::test]
async fn the_resource_field_keeps_its_colons_for_both_readers() {
    let router = emulator();
    let resources = [
        ("a plain resource", "role/AIOpsCrossAccountRole"),
        ("a resource at a path", "role/team/ops/Reader"),
        ("a resource carrying colons", "role:scoped:Reader"),
        ("a resource carrying both", "role:scoped/team/Reader"),
        ("a single-token resource", "root"),
    ];

    for (index, (case, resource)) in resources.iter().enumerate() {
        let home = region(index);
        let arn = create_target(&router, &home).await;
        let role = format!("arn:aws:iam::{FOREIGN_ACCOUNT}:{resource}");

        // The account is readable whatever the resource portion holds, so the write is
        // accepted and stores the list as sent.
        let (status, updated) = call_in(
            &router,
            &home,
            "PATCH",
            &format!("/investigationGroups/{}", encode(&arn)),
            Some(&json!({ "crossAccountConfigurations": entries(&role) })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {role:?} names {FOREIGN_ACCOUNT}, so the update must be \
             accepted, got {status} / {updated:?}"
        );
        let (status, after) = call_in(
            &router,
            &home,
            "GET",
            &format!("/investigationGroups/{}", encode(&arn)),
            None,
        )
        .await;
        assert_eq!(
            (status, after.get("crossAccountConfigurations")),
            (200, Some(&entries(&role))),
            "case {case:?}: expected the read-back to report {:?}, got {status} / \
             {after:?}",
            entries(&role)
        );

        // And the same resource portion, appended to the group's own resource field,
        // addresses no group: the sixth field keeps its colons rather than being split
        // down to the group's id.
        let probe = format!("{arn}:{resource}");
        let (status, tags) = call_in(
            &router,
            &home,
            "GET",
            &format!("/tags/{}", encode(&probe)),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&tags).as_str()),
            (404, TAGGING_NOT_FOUND),
            "case {case:?}: {probe:?} addresses no group, got {status} / {tags:?}"
        );
    }
}
