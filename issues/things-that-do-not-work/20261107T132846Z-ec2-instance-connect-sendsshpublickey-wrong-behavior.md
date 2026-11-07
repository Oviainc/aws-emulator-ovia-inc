Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e4e3e5befb4453536c7145f8d5553fe1cd0770b8

# Resource-tag condition keys are published as a KNOWN tag set for an instance record whose tags nothing reported, so a tag-conditioned Deny stops refusing

_Filed: 2026-11-07T13:28:46Z_
_Operation: SendSSHPublicKey | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e4e3e5befb4453536c7145f8d5553fe1cd0770b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both handlers build the authorization condition context with `Conditions::new(.., found.as_ref().map(|instance| &instance.tags))`, i.e. the tag condition families (`aws:ResourceTag/` and `ec2:ResourceTag/`) are published whenever an instance RECORD was resolved at all. The crate documents a different rule: the families are published only for a tag set that is KNOWN, because publishing a family declares, as a fact, that every key of it that is absent is a tag the instance does not carry.

`crate::state::Instance` cannot represent the difference the doc asserts: `tags` is a plain `BTreeMap`, and `Instance::new` initialises it EMPTY — that is the constructor every record of the fallback registry (`seed_instances`) is built with, and the doc on the field calls such a record one "whose tags nothing reported", explicitly "not the same as an instance with no tags". The same flattening happens for a tag Amazon EC2 DID report but with no value, which `state::ec2::instance` deliberately drops from the set before the set is published as complete.

Consequence, by the crate's own stated evaluation semantics: a published-but-absent key makes a clause over it decide FALSE, while an undeclared key leaves the statement UNDECIDED, which the crate describes as fail-closed in both directions ("a `Deny` that may hold still refuses"). So in an emulator with no EC2 mounted — the documented reason the fallback registry exists — a push at a seeded instance under an identity policy that carries `Allow *` plus a `Deny` conditioned on `aws:ResourceTag/Environment` is ALLOWED and the key is stored, where the rule the crate states for an unknown tag set requires the Deny to refuse it. The `None` arm of `Conditions::new` (the fail-closed arm) is in practice reachable only when no instance was found at all, i.e. only for a push that is about to be refused as `EC2InstanceNotFoundException` anyway.

## Evidence

The rule the crate states — `src/state.rs:352-361`:
```
    /// The instance's tags, as EC2 reports them (`Instance$Tags`), keyed by tag key.
    ...
    /// from whoever answered for the instance rather than invented here. A record whose tags
    /// nothing reported carries none, which is not the same as an instance with no tags —
    /// see [`authorization::Conditions::new`], which only publishes the tag condition keys
    /// for a tag set that IS known.
    pub tags: BTreeMap<String, String>,
```
and `src/state/authorization.rs:166-174`:
```
    /// The condition context of one push: the OS user it names, where the operation takes one,
    /// and the tags of the instance it addresses, where the tag set is KNOWN in full.
    ///
    /// The tag families are published only for a `tags` the owner of the instance answered
    /// with: a key of the family that is then absent is a tag the instance does not carry,
    /// which is a fact. For an instance nothing could answer for, the tag set is not known —
    /// not known to be empty — so neither family is published and a clause over one of them
    /// leaves its statement undecided.
```
with the fail-closed semantics it relies on, `src/state/authorization.rs:150-157`: "A key declared here and absent from the values is one the request carries no value for, which is what lets a clause over it be decided at all; every OTHER key stays undeclared, and the shared evaluation leaves a clause over one of those UNDECIDED — an `Allow` that may not hold grants nothing and a `Deny` that may hold still refuses, which is fail-closed in both directions."

The call site that cannot honour it — `src/ops/send_ssh_public_key.rs:249-252`:
```
        let conditions = crate::state::authorization::Conditions::new(
            Some(&input.instance_os_user),
            found.as_ref().map(|instance| &instance.tags),
        );
```
(the sibling handler does the same at `src/ops/send_serial_console_ssh_public_key.rs:250-253`), and the publishing branch it reaches, `src/state/authorization.rs:184-195`:
```
        if let Some(tags) = tags {
            for (key, value) in tags { ... }
            conditions
                .published
                .extend_from_slice(&[AWS_RESOURCE_TAG_PREFIX, EC2_RESOURCE_TAG_PREFIX]);
        }
```

Why "nothing reported" is indistinguishable from "no tags" — `src/state.rs:372-380` (`Instance::new`):
```
            state,
            serial_console_supported,
            tags: BTreeMap::new(),
```
and that is the constructor the fallback registry uses, `src/state.rs:505-514` (`seed_instances`, `Instance::new("i-0a1b2c3d4e5f60718", "us-east-1a", "m5.large", InstanceState::Running, true)` — no `with_tags`). A second flattening: `src/state/ec2.rs:146-157` drops a tag EC2 reported without a value — "A tag EC2 reports without a key or without a value is not a `(key, value)` pair an authorization clause can be decided against, so it is left out rather than filled in with an empty string" — after which the reduced set is still published as the complete, known one.

Test coverage: the unit test `a_clause_over_a_published_key_is_decided_from_the_request` (`src/state/authorization.rs:511-542`) pins the two inputs as DIFFERENT cases — `("no tag of that key", Some("ec2-user"), Some(&BTreeMap::new()), Decision::NotStated)` and `("a tag set nothing answered for", Some("ec2-user"), None, Decision::NotStated)` — but both rows are `Allow` statements, so neither pins the call site's choice, and no test anywhere in the crate exercises a `Deny` conditioned on a resource tag (`tests/all/identity_policy_authorization.rs:539-590` uses `Allow` rows only, against an instance registered WITH tags at `:287-290` `.with_tags([(TAG_KEY, TAG_VALUE)])`).

## Steps to Reproduce

1. Mount this service with IAM and STS on one engine and NO EC2, exactly as `tests/all/identity_policy_authorization.rs:97-107` does, so an instance is resolved from this service's fallback registry (`seed_instances`).
2. Pick the seeded running Nitro instance `i-0a1b2c3d4e5f60718` (`src/state.rs:508-514`), whose record was built by `Instance::new` and therefore carries an EMPTY `tags` map that nothing reported.
3. Create an IAM user through `CreateUser`, give it an inline policy with two statements — `{"Effect":"Allow","Action":"ec2-instance-connect:SendSSHPublicKey","Resource":"*"}` and `{"Effect":"Deny","Action":"ec2-instance-connect:SendSSHPublicKey","Resource":"*","Condition":{"StringEquals":{"aws:ResourceTag/Environment":"staging"}}}` — and mint an access key for it.
4. POST `/` with `X-Amz-Target: AWSEC2InstanceConnectService.SendSSHPublicKey`, that key id in the SigV4 credential scope, and body `{"InstanceId":"i-0a1b2c3d4e5f60718","InstanceOSUser":"ec2-user","SSHPublicKey":"<80+ char key>"}`.
5. Observed: HTTP 200 with `Success: true` and the key stored in `pushed_keys` — the tag families were published for a tag set nothing reported, so the Deny's clause decided false.
6. Expected per the rule the crate states for a tag set that is not known (`src/state/authorization.rs:170-174` and `:150-157`): neither family is published, the Deny's statement is left undecided, and a `Deny` that may hold still refuses the push.
