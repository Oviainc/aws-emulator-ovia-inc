// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` end to end over HTTP: whose ARN a tag read answers for, and
//! what it answers.
//!
//! The rules under test, asserted over TABLES rather than one recorded request:
//!
//! 1. The answer is a projection of the addressed record's stored tags — the tags a
//!    create persisted, and an EMPTY map for a record carrying none, which is not the
//!    same answer as the member being absent.
//! 2. `resourceArn` is resolved by PARSING it, the way every tagging operation of this
//!    service resolves it: a value that is not an ARN of this service naming
//!    `managed-device/<id>` or `task/<id>` is refused HTTP 400 `BadRequestException`; an
//!    ARN whose account is not the one this emulator serves is refused 403
//!    `AccessDeniedException` whatever it names, so a tag read cannot be used to learn
//!    what another account holds; and an ARN of this account addressing no stored record
//!    is refused 404 `NotFoundException`.
//! 3. A read changes nothing a later caller sees, and neither does a refusal.
//!
//! Rows differ along what the rules generalise over: the resource kind, the number and
//! form of the tags, the ARN field that is wrong, the account that owns the ARN, and —
//! the no-leak invariant — whether the resource a foreign ARN names exists in THIS
//! account at all.
//!
//! Every case goes through the router, so nothing reaches into the state struct and an
//! answer that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{
    call, create_task, describe_device, describe_task, emulator, ids_of, ok, string,
    whole_inventory, Answer, ACCOUNT, SIGNED_REGION,
};

/// An account id that is not the one this emulator serves.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// This service's ARN namespace / sigv4 signing name, from
/// `smithy_query(query="service_info")`.
const ARN_SERVICE: &str = "snow-device-management";

/// The modelled bound on `ManagedDeviceId` and `TaskId` (`@length(min: 1, max: 64)`),
/// spelled out here rather than imported so a wrong bound in the implementation cannot
/// make this test pass.
const RESOURCE_ID_CEILING: usize = 64;

/// How this service words a `resourceArn` its tagging APIs cannot parse, as the
/// behavioural corpus recorded it. Asserted on EVERY malformed row, so the wording has
/// to state the refusal class instead of being fitted to one request.
const INVALID_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// How this service words a `resourceArn` that parses but addresses no device and no
/// task, as the behavioural corpus recorded it. Asserted on every not-found row, so the
/// wording has to state the class rather than one request's resource.
const NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The caller a request signed with this emulator's long-term credentials speaks for:
/// STS `GetCallerIdentity` answers `arn:aws:iam::<account>:user/emulator` for every
/// request whose access key id names no session. Spelled out here rather than imported,
/// so a refusal that names some other principal cannot make this suite pass.
const CALLER_PRINCIPAL_USER: &str = "emulator";

/// An ARN built field by field, so a row can make exactly one field wrong.
fn arn(service: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{account}:{resource}")
}

/// `resourceArn` is bound as an `@httpLabel` on `GET /tags/{resourceArn}`, and a
/// non-greedy label is ONE path segment: the `/` inside an ARN's resource field has to
/// be written `%2F` or it would split the segment. `:` is a legal path character, so it
/// is sent as-is here and [`fully_encoded`] covers the other spelling.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// The same ARN with every reserved character percent-encoded, as an SDK writes it.
fn fully_encoded(resource_arn: &str) -> String {
    resource_arn.replace(':', "%3A").replace('/', "%2F")
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(resource_arn)), None).await
}

/// The `tags` member of a `ListTagsForResource` answer. Required to be present: a
/// resource with no tags has an EMPTY list of them.
fn tags_of(answer: &Answer, case: &str) -> Value {
    let body = ok(answer, case);
    let tags = body
        .get("tags")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags member, got {body}"));
    assert!(
        tags.is_object(),
        "case {case:?}: expected tags to be a map, got {tags}"
    );
    tags
}

/// The `tags` member of ANOTHER reader's report of the same record, required to be
/// present: comparing a default against a default would pass whether that reader
/// reports an empty map or omits the member altogether, so it would assert nothing.
fn reported_tags(body: &Value, case: &str) -> Value {
    let tags = body
        .get("tags")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags member, got {body}"));
    assert!(
        tags.is_object(),
        "case {case:?}: expected tags to be a map, got {body}"
    );
    tags
}

/// The bare name of the wire error code, whichever form the protocol renders it in
/// (`NotFoundException`, or a shape id ending in it). Compared whole, because a client
/// branches on the code and `ResourceNotFoundException` ends in `NotFoundException`.
fn error_code_name(answer: &Answer) -> &str {
    let code = answer.error_code();
    code.rsplit(['#', '/']).next().unwrap_or(code)
}

/// The error message, or the empty string when there is none.
fn message_of(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// Assert a refusal carried `status`, exactly the error code `code`, exactly `message`,
/// and nothing about the ARN it was asked about.
fn assert_refused(answer: &Answer, status: u16, code: &str, message: &str, case: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status} {code}, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code_name(answer),
        code,
        "case {case:?}: expected the error code {code}, got {}",
        answer.body
    );
    assert_eq!(
        message_of(answer),
        message,
        "case {case:?}: expected the one class message for this refusal, got {}",
        answer.body
    );
}

/// Assert a `resourceArn` that is not one of this service's ARNs was refused 400
/// `BadRequestException`, disclosing nothing about the value.
fn assert_bad_request(answer: &Answer, case: &str, resource_arn: &str) {
    assert_refused(answer, 400, "BadRequestException", INVALID_ARN_MESSAGE, case);
    assert_no_echo(answer, case, resource_arn);
}

/// Assert an ARN of this account that addresses no stored record was refused 404
/// `NotFoundException`, disclosing nothing about the ARN.
fn assert_not_found(answer: &Answer, case: &str, resource_arn: &str) {
    assert_refused(answer, 404, "NotFoundException", NOT_FOUND_MESSAGE, case);
    assert_no_echo(answer, case, resource_arn);
}

/// Assert an ARN of another account was refused 403 `AccessDeniedException` naming the
/// CALLER's own principal and nothing about the resource it was asked about.
fn assert_denied(answer: &Answer, case: &str, resource_arn: &str) {
    let expected = format!(
        "User: arn:aws:iam::{ACCOUNT}:user/{CALLER_PRINCIPAL_USER} \
         is not authorized to access this resource"
    );
    assert_refused(answer, 403, "AccessDeniedException", &expected, case);
    assert_no_echo(answer, case, resource_arn);
    assert!(
        !answer.body.to_string().contains(FOREIGN_ACCOUNT),
        "case {case:?}: the refusal must not echo the account it was asked about, got {}",
        answer.body
    );
}

/// Every refusal on this path discloses nothing about the ARN it was asked about.
fn assert_no_echo(answer: &Answer, case: &str, resource_arn: &str) {
    assert!(
        !answer.body.to_string().contains(resource_arn),
        "case {case:?}: the refusal must not echo {resource_arn:?}, got {}",
        answer.body
    );
}

/// The first seeded device's id and stored ARN.
async fn a_seeded_device(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let inventory = whole_inventory(router, case).await;
    let id = ids_of(&inventory, case)
        .into_iter()
        .next()
        .expect("the emulator seeds a device inventory");
    let arn = string(&inventory[0], "managedDeviceArn", case);
    (id, arn)
}

/// Create a task over the first seeded device with `tags`, returning its id and ARN.
async fn tagged_task(router: &MultiServiceRouter, case: &str, tags: &Value) -> (String, String) {
    let (device_id, _) = a_seeded_device(router, case).await;
    let created = ok(
        &create_task(
            router,
            json!({ "targets": [device_id], "command": { "unlock": {} }, "tags": tags }),
        )
        .await,
        case,
    );
    (
        string(&created, "taskId", case),
        string(&created, "taskArn", case),
    )
}

/// Rule 1, on tasks: the answer is the tags the create persisted.
#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_the_create_persisted() {
    // Rows differ in how many tags the resource carries and in the form of the keys and
    // values. `TagMap` is a plain `map<String, String>` with no modelled constraint
    // (`smithy_query(query="shape", shape="TagMap")`), so every one of these is a value
    // the model admits.
    let rows: &[(&str, Value)] = &[
        ("no tags at all", json!({})),
        ("one tag", json!({ "team": "networking" })),
        (
            "three tags",
            json!({ "team": "networking", "env": "prod", "owner": "ops" }),
        ),
        (
            "a namespaced key and an empty value",
            json!({ "example.com/cost-centre": "" }),
        ),
        (
            "a value carrying spaces and non-ASCII text",
            json!({ "description": "côté nord — rack 12" }),
        ),
    ];

    for (case, tags) in rows {
        let router = emulator();
        let (task_id, task_arn) = tagged_task(&router, case, tags).await;

        // Read the tags back through the operation under test...
        assert_eq!(
            &tags_of(&list_tags(&router, &task_arn).await, case),
            tags,
            "case {case:?}: ListTagsForResource must report the tags CreateTask persisted"
        );
        // ...and through the other reader of the same record, which must agree.
        let described = ok(&describe_task(&router, &task_id).await, case);
        assert_eq!(
            reported_tags(&described, case),
            *tags,
            "case {case:?}: DescribeTask must report the same tags, got {described}"
        );
        // Rule 3: a read changes nothing, so the second answer is the first.
        assert_eq!(
            &tags_of(&list_tags(&router, &task_arn).await, case),
            tags,
            "case {case:?}: a repeated ListTagsForResource must answer the same tags"
        );
    }
}

/// Rule 1, on devices: the seeded inventory carries no tags, which is an empty map and
/// not an absent member.
#[tokio::test]
async fn list_tags_for_resource_reports_an_empty_map_for_every_untagged_device() {
    let router = emulator();
    let inventory = whole_inventory(&router, "inventory").await;
    for device in &inventory {
        let case = format!("device {device}");
        let id = string(device, "managedDeviceId", &case);
        let device_arn = string(device, "managedDeviceArn", &case);

        assert_eq!(
            tags_of(&list_tags(&router, &device_arn).await, &case),
            json!({}),
            "case {case:?}: a device carrying no tags has an empty tag map"
        );
        // The device reader agrees, so both answers come from the one stored record.
        let described = ok(&describe_device(&router, &id).await, &case);
        assert_eq!(
            reported_tags(&described, &case),
            json!({}),
            "case {case:?}: DescribeDevice must report an empty tags map too, got {described}"
        );
    }
}

#[tokio::test]
async fn a_fully_percent_encoded_label_names_the_same_arn() {
    // The same ARN in the spelling an SDK sends (`:` as `%3A`) must resolve to the same
    // record as the spelling that leaves `:` alone.
    let router = emulator();
    let tags = json!({ "spelling": "encoded" });
    let (_, task_arn) = tagged_task(&router, "percent-encoded label", &tags).await;

    let answer = call(
        &router,
        "GET",
        &format!("/tags/{}", fully_encoded(&task_arn)),
        None,
    )
    .await;
    assert_eq!(
        tags_of(&answer, "percent-encoded label"),
        tags,
        "a fully percent-encoded resourceArn must name the same task"
    );
}

#[tokio::test]
async fn an_arn_naming_another_region_of_this_account_addresses_the_same_record() {
    // The ARN's region is not part of the key: the inventory and the task store are
    // per-account, so an ARN of this account naming another region addresses the same
    // record — the same resolution `UntagResource` performs.
    let router = emulator();
    let tags = json!({ "scope": "per-account" });
    let (task_id, _) = tagged_task(&router, "another region", &tags).await;
    let (device_id, _) = a_seeded_device(&router, "another region").await;

    let rows: &[(&str, String)] = &[
        (
            "the task under another region",
            arn(ARN_SERVICE, "eu-west-1", ACCOUNT, &format!("task/{task_id}")),
        ),
        (
            "the device under another region",
            arn(
                ARN_SERVICE,
                "us-east-1",
                ACCOUNT,
                &format!("managed-device/{device_id}"),
            ),
        ),
    ];
    let expected = [tags.clone(), json!({})];
    for ((case, resource_arn), want) in rows.iter().zip(expected) {
        assert_eq!(
            tags_of(&list_tags(&router, resource_arn).await, case),
            want,
            "case {case:?}: an ARN of this account must address the same per-account record"
        );
    }
}

/// Rule 2, the denied half, and the no-leak invariant it exists for.
#[tokio::test]
async fn list_tags_for_resource_denies_every_arn_of_another_account() {
    let router = emulator();
    let (device_id, device_arn) = a_seeded_device(&router, "inventory").await;
    let (task_id, task_arn) = tagged_task(&router, "a task to deny", &json!({ "k": "v" })).await;

    // Rows differ in the resource kind, in whether THIS account holds a record of that
    // id, in the region and the partition the ARN names, and in how far the account id
    // is from the caller's own.
    let rows: Vec<(String, String)> = vec![
        (
            "another account, a device id this account does hold".to_string(),
            arn(
                ARN_SERVICE,
                SIGNED_REGION,
                FOREIGN_ACCOUNT,
                &format!("managed-device/{device_id}"),
            ),
        ),
        (
            "another account, a device id nothing holds".to_string(),
            arn(
                ARN_SERVICE,
                SIGNED_REGION,
                FOREIGN_ACCOUNT,
                "managed-device/smd-ffffffffffffffff",
            ),
        ),
        (
            "another account, a task id this account does hold".to_string(),
            arn(
                ARN_SERVICE,
                SIGNED_REGION,
                FOREIGN_ACCOUNT,
                &format!("task/{task_id}"),
            ),
        ),
        (
            "another account, in another region".to_string(),
            arn(
                ARN_SERVICE,
                "eu-west-1",
                FOREIGN_ACCOUNT,
                &format!("managed-device/{device_id}"),
            ),
        ),
        (
            "another account, in another partition".to_string(),
            format!(
                "arn:aws-cn:{ARN_SERVICE}:cn-north-1:{FOREIGN_ACCOUNT}:managed-device/{device_id}"
            ),
        ),
        (
            "an account id one digit from the caller's own".to_string(),
            arn(
                ARN_SERVICE,
                SIGNED_REGION,
                "000000000001",
                &format!("managed-device/{device_id}"),
            ),
        ),
    ];

    for (case, resource_arn) in &rows {
        assert_denied(&list_tags(&router, resource_arn).await, case, resource_arn);
    }

    // The no-leak invariant: the refusal for a foreign ARN naming a resource this
    // account DOES hold is identical to the one for a foreign ARN of the same form
    // naming nothing, so a caller learns nothing about the other account's inventory.
    let held = list_tags(
        &router,
        &arn(
            ARN_SERVICE,
            SIGNED_REGION,
            FOREIGN_ACCOUNT,
            &format!("managed-device/{device_id}"),
        ),
    )
    .await;
    let absent = list_tags(
        &router,
        &arn(
            ARN_SERVICE,
            SIGNED_REGION,
            FOREIGN_ACCOUNT,
            "managed-device/smd-ffffffffffffffff",
        ),
    )
    .await;
    assert_eq!(
        (held.status, &held.body),
        (absent.status, &absent.body),
        "a foreign ARN must be refused identically whether or not it names a resource \
         this account holds"
    );

    // Rule 3: the refusals changed nothing, so both records still read back as they were.
    assert_eq!(
        tags_of(
            &list_tags(&router, &task_arn).await,
            "the task after the refusals"
        ),
        json!({ "k": "v" }),
        "a refused call must not disturb the task's tags"
    );
    assert_eq!(
        tags_of(
            &list_tags(&router, &device_arn).await,
            "the device after the refusals"
        ),
        json!({}),
        "a refused call must not disturb the device's tags"
    );
}

/// Rule 2, the not-found half: an ARN of this account that parses but addresses no
/// stored record.
#[tokio::test]
async fn list_tags_for_resource_404s_an_own_account_arn_that_addresses_no_record() {
    let router = emulator();
    let (device_id, device_arn) = a_seeded_device(&router, "inventory").await;
    let (task_id, task_arn) = tagged_task(&router, "a task to miss", &json!({})).await;

    // Rows stay inside the caller's own account and inside the modelled id bound — so
    // the ARN parses and the caller is authorized — and differ in WHY nothing carries
    // the id: never minted, the id of the OTHER resource kind, the length ceiling
    // itself, and a near miss on a real id.
    let longest_id = "s".repeat(RESOURCE_ID_CEILING);
    let rows: Vec<(String, String)> = vec![
        (
            "a managed-device id that was never seeded".to_string(),
            "managed-device/smd-ffffffffffffffff".to_string(),
        ),
        (
            "a task id that was never minted".to_string(),
            "task/st-ffffffffffffffff".to_string(),
        ),
        (
            "an id at the modelled length ceiling".to_string(),
            format!("task/{longest_id}"),
        ),
        (
            "a task id carried under the device resource type".to_string(),
            format!("managed-device/{task_id}"),
        ),
        (
            "a device id carried under the task resource type".to_string(),
            format!("task/{device_id}"),
        ),
        (
            "a device id with one extra character".to_string(),
            format!("managed-device/{device_id}0"),
        ),
    ];

    for (case, resource) in &rows {
        let resource_arn = arn(ARN_SERVICE, SIGNED_REGION, ACCOUNT, resource);
        assert_not_found(&list_tags(&router, &resource_arn).await, case, &resource_arn);
    }

    // Rule 3, and the positive control for the table: the records this account does hold
    // are still served, so the refusals above are about the ARN and not the operation.
    assert_eq!(
        tags_of(&list_tags(&router, &device_arn).await, "the device"),
        json!({}),
        "a not-found call must not disturb the stored device"
    );
    assert_eq!(
        tags_of(&list_tags(&router, &task_arn).await, "the task"),
        json!({}),
        "a not-found call must not disturb the stored task"
    );
}

/// Rule 2, the malformed half: a value that is not an ARN of this service naming a
/// device or a task never addressed a resource here.
#[tokio::test]
async fn list_tags_for_resource_400s_a_resource_arn_it_cannot_parse() {
    let router = emulator();
    let (device_id, device_arn) = a_seeded_device(&router, "inventory").await;
    let (task_id, _) = tagged_task(&router, "a task to mis-address", &json!({})).await;

    // Rows differ along WHICH field is wrong — the `arn` prefix, the field count, the
    // partition, the service, the region, the account, the resource type, the id —
    // because the rule is about parsing the ARN, not about one recorded string. The
    // id-length row sits one past the modelled `@length(min: 1, max: 64)` ceiling of
    // `ManagedDeviceId` / `TaskId`; the ceiling itself parses and is a row of the
    // not-found table above.
    let too_long_id = "s".repeat(RESOURCE_ID_CEILING + 1);
    let rows: Vec<(String, String)> = vec![
        ("not an ARN at all".to_string(), "this-is-not-an-arn".to_string()),
        ("the resource id alone".to_string(), task_id.clone()),
        (
            "the arn prefix miscased".to_string(),
            format!("ARN:aws:{ARN_SERVICE}:{SIGNED_REGION}:{ACCOUNT}:task/{task_id}"),
        ),
        (
            "an empty partition".to_string(),
            format!("arn::{ARN_SERVICE}:{SIGNED_REGION}:{ACCOUNT}:task/{task_id}"),
        ),
        (
            "too few fields".to_string(),
            format!("arn:aws:{ARN_SERVICE}:{SIGNED_REGION}:{ACCOUNT}"),
        ),
        (
            "another service's ARN in this account".to_string(),
            arn(
                "ec2",
                SIGNED_REGION,
                ACCOUNT,
                &format!("instance/{device_id}"),
            ),
        ),
        (
            "an empty region".to_string(),
            arn(ARN_SERVICE, "", ACCOUNT, &format!("task/{task_id}")),
        ),
        (
            "an account that is not 12 digits".to_string(),
            arn(ARN_SERVICE, SIGNED_REGION, "12345", &format!("task/{task_id}")),
        ),
        (
            "an account that is not digits".to_string(),
            arn(
                ARN_SERVICE,
                SIGNED_REGION,
                "00000000000a",
                &format!("task/{task_id}"),
            ),
        ),
        (
            "an ARN carrying no account at all".to_string(),
            arn(ARN_SERVICE, SIGNED_REGION, "", &format!("task/{task_id}")),
        ),
        (
            "a resource type this service does not have".to_string(),
            arn(ARN_SERVICE, SIGNED_REGION, ACCOUNT, &format!("job/{task_id}")),
        ),
        (
            "a resource with no id".to_string(),
            arn(ARN_SERVICE, SIGNED_REGION, ACCOUNT, "task/"),
        ),
        (
            "a resource with no type separator".to_string(),
            arn(ARN_SERVICE, SIGNED_REGION, ACCOUNT, "task"),
        ),
        (
            "an id one past the modelled length ceiling".to_string(),
            arn(
                ARN_SERVICE,
                SIGNED_REGION,
                ACCOUNT,
                &format!("managed-device/{too_long_id}"),
            ),
        ),
    ];

    for (case, resource_arn) in &rows {
        assert_bad_request(&list_tags(&router, resource_arn).await, case, resource_arn);
    }

    // Rule 3, and the positive control: the device's own ARN is still served.
    assert_eq!(
        tags_of(&list_tags(&router, &device_arn).await, "the device"),
        json!({}),
        "a refused call must not disturb the stored device"
    );
}
