Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# The @idempotent creation fingerprint cannot tell an absent `tags` member from an empty one, although it distinguishes exactly that for the other two optional members and the crate states omitting a member and supplying it are different requests

_Filed: 2026-10-29T18:17:23Z_
_Operation: CreateDomain | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDomain` is modelled `@idempotent`, and this crate decides what counts as a REPEAT by digesting the request's parameters into `crate::state::domain_creation_fingerprint`. That function's own doc states the rule it follows for optional members: "An absent optional member contributes a part of its own, so omitting a member and supplying it are different requests." It keeps that rule for the two OTHER optional members — an absent `kmsKeyArn` pushes an empty part where a supplied one pushes `kmsKeyArn:<arn>`, and an absent `webAppSetupConfiguration` pushes an empty part where a supplied one pushes three — but it breaks it for `tags`: the only parts `tags` contributes are the entry count and the entries themselves, so an ABSENT `tags` and a SUPPLIED-but-EMPTY `tags` both contribute exactly `"0"` and nothing else.

The handler erases the distinction before the digest is even taken: `create_domain.rs` calls `accepted_entries(input.tags.unwrap_or_default())`, which maps `None` and `Some({})` onto the same empty `BTreeMap`, and that map is what is handed to `domain_creation_fingerprint`.

Consequence on the wire: `POST /domain {"name":"clinic"}` followed by `POST /domain {"name":"clinic","tags":{}}` is served as an `@idempotent` repeat — the second call answers 201 with the FIRST domain's `domainId`, `arn` and `createdAt`, mints nothing, and consumes no further slot of the account's domain cap — even though the two requests differ in which members they carry. `create_domain.rs`'s own module header states the opposite outcome: "a request differing in any parameter is a different request and creates its own domain."

The defect is the divergence between the stated rule and the digest: either the digest must fold in a part for an absent `tags` the way it does for the other two optional members, or the two doc sentences (and the suite's generalisation, which claims to cover "the two ways an optional member can differ: a different value, and no value at all") overstate what the fingerprint does. As written the crate promises a distinction it does not make, and the three optional members of one operation are identified by two different rules.

## Evidence

The rule the fingerprint states for itself — services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:546-550:
```
/// `tags` is digested in KEY order, the order the store holds the set in, so the same
/// entries presented in a different wire order are the same request; the entry count is
/// folded in ahead of them so the sections cannot be read into one another. An absent
/// optional member contributes a part of its own, so omitting a member and supplying it
/// are different requests.
```

The code under it — src/state.rs:556-575. The two sibling optional members push a part when ABSENT; `tags` pushes only its count and its entries, so absent and empty are byte-identical inputs to `fingerprint_of`:
```
    let mut parts: Vec<String> = vec![name.to_string()];
    match kms_key_arn {
        Some(arn) => parts.push(format!("kmsKeyArn:{arn}")),
        None => parts.push(String::new()),
    }
    parts.push(tags.len().to_string());
    for (key, value) in tags {
        parts.push(key.clone());
        parts.push(value.clone());
    }
    match web_app_setup {
        Some(setup) => {
            parts.push(setup.ehr_role.clone());
            parts.push(setup.idc_instance_id.clone());
            parts.push(setup.idc_region.clone());
        }
        None => parts.push(String::new()),
    }
    fingerprint_of(parts.iter().map(String::as_str))
```

The handler collapses `None` into `{}` before the digest — src/ops/create_domain.rs:295-297 and :311-316:
```
        let requested_tags =
            crate::service::tag_bounds::accepted_entries(input.tags.unwrap_or_default())
                .map_err(crate::scaffolded::Scaffolded::validation)?;
```
```
        let fingerprint = crate::state::domain_creation_fingerprint(
            &input.name,
            input.kms_key_arn.as_deref(),
            &requested_tags,
            input.web_app_setup_configuration.as_ref(),
        );
```

The outcome the same handler's header promises — src/ops/create_domain.rs:8-15:
```
//! The operation is modelled `@idempotent`
//! (`smithy_query(query="operation",operation="CreateDomain")` reports
//! `"idempotent": true`), so a REPEAT of a request this account already created a live
//! domain with in this Region resolves to that domain and mints nothing: the effect of
//! the repeated call is the effect of the single call. The request's identity is the
//! digest of the parameters it carried ([`crate::state::domain_creation_fingerprint`]),
//! stored on the record it created, so a request differing in any parameter is a
//! different request and creates its own domain.
```

Where the repeat is resolved from that digest — src/ops/create_domain.rs:323-325:
```
        if let Some(existing) = store.domain_created_by(&region, &account_id, &fingerprint) {
            return Ok(rendered(existing));
        }
```

The model confirms `tags` is an optional member whose presence is a property of the request — `smithy_query(query="operation", operation="CreateDomain")`:
```
"tags": {"target": "TagMap", "notProperty": true, "type": "map"}
```
(no `@required`, no `@default`), exactly like `kmsKeyArn` and `webAppSetupConfiguration`, which the digest DOES distinguish.

Test coverage — the suite asserts the generalisation but never this pair. tests/all/create_domain_idempotent_repeat.rs:164-174:
```
async fn a_create_differing_in_any_one_parameter_is_a_different_request() {
    let base = json!({
        "name": "clinic",
        "kmsKeyArn": kms_key("abcdef01-2345-6789-abcd-ef0123456789"),
        "tags": { "env": "prod" },
        "webAppSetupConfiguration": default_web_app(),
    });
    // One row per parameter `CreateDomain` accepts, each differing from `base` in exactly
    // that parameter — including the two ways an optional member can differ: a different
    // value, and no value at all.
```
Its `tags` "no value at all" row (tests/all/create_domain_idempotent_repeat.rs:229-236) is measured against a base carrying `{"env":"prod"}`, so it only proves count 0 differs from count 1; and the identical-repeat table's empty-map row (tests/all/create_domain_idempotent_repeat.rs:84-86) always sends `"tags": {}` on BOTH calls:
```
            "no tags",
            json!({ "name": "clinic-no-tags", "tags": {} }),
```
No case sends an absent `tags` and then an empty `tags`, so the collapsed spelling is unpinned.

## Steps to Reproduce

1. Read services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:546-550 — the fingerprint states that omitting an optional member and supplying it are different requests — and src/ops/create_domain.rs:8-15, which states that a request differing in any parameter creates its own domain.
2. Read src/state.rs:556-575: an absent `kmsKeyArn` and an absent `webAppSetupConfiguration` each push a part of their own, while `tags` pushes only `tags.len()` and its entries — so absent and empty both yield the single part "0".
3. Read src/ops/create_domain.rs:295-297: `input.tags.unwrap_or_default()` maps `None` and `Some({})` onto the same empty map before the digest at :311-316 is taken.
4. Send POST /domain with body {"name":"clinic"} and record the returned domainId and createdAt.
5. Send POST /domain with body {"name":"clinic","tags":{}} — a request carrying a member the first did not.
6. Expected per src/state.rs:546-550 and src/ops/create_domain.rs:14-15: a different request, so a second domain with a new domainId. Actual: 201 carrying the FIRST domain's domainId, arn and createdAt, with nothing minted (src/ops/create_domain.rs:323-325 resolves the repeat), and GET /domain shows one domain, not two.
7. Confirm the inverse ordering too: POST /domain {"name":"clinic2","tags":{}} followed by POST /domain {"name":"clinic2"} is likewise served as a repeat; and confirm with tests/all/create_domain_idempotent_repeat.rs:84-86 and :229-236 that no existing case sends absent-then-empty, so no test pins the current answer.
