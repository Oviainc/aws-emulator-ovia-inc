Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService

# The enum-constraint refusal lists an enum's admitted values in another order than the service does

_Filed: 2026-10-24T03:30:00Z_
_Operation: PutAccountDataRetention (every enum-constrained member of every operation) | Class: wrong-behavior_

> _Note: filed while implementing `GetAccountDataRetention` / `PutAccountDataRetention`
> (`issues/things-that-now-work/20261024T025841Z-bedrock-getaccountdataretention-stub.md`).
> Both operations answered HTTP 501 before that, so no request reached the refusal this
> issue is about; it became observable only once they were implemented. The refusal is not
> data-retention code: it is the crate-wide envelope in `src/service/validation.rs`, so
> this holds for every enum-constrained member of every operation this crate serves._

## Description

A request carrying a value outside a modelled enum is refused with the right status, the
right code and the right clause — HTTP 400 `ValidationException`,
`1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must
satisfy enum value set: [ … ]` — but the admitted values inside the brackets are listed in
a different ORDER than Amazon Bedrock lists them in.

The clause lists them as codegen captured them off the model, which this crate's generated
`*_VALUES` constants spell alphabetically: for `DataRetentionMode`,
`[aws_review, default, inherit, none, provider_data_share]`. The behavioural corpus records
Bedrock listing `[default, provider_data_share, none, aws_review, inherit]` — which is
neither that alphabetical order nor the model's declaration order
(`smithy_query(query="shape",shape="DataRetentionMode")` reports
`default, none, aws_review, provider_data_share, inherit`).

So no order derivable from the model produces the recorded one, and nor does the bucket
order a hash set of those five strings iterates in (`default, aws_review, inherit, none,
provider_data_share`, by the usual string hash over a 16-bucket table), which is the other
candidate a service listing a value set might show.

Note also what the clause CAN render today. The order it lists is the one codegen captured
onto the member schema's enum trait, and codegen reads the model's enum members out of a
sorted map (`vendor/smithy-macros/src/codegen.rs::enum_shape_values` iterates a
`serde_json::Map` built without `preserve_order`), so the values arrive alphabetically by
member NAME and the model's declaration order is not available to this crate at run time at
all. Rendering a declared order would therefore need that vendored reader to preserve it —
`vendor/` is read-only from inside the emulator — and even then it would not match the
recording above.

So whoever picks this up has to establish what rule Bedrock lists an enum's values by —
over several enums of this service, not just this one, since one recorded message cannot
distinguish a rule from a coincidence — and then apply that rule in the one place the clause
is built, which may first need the vendored capture to carry an order worth applying it to.
Pasting this one recording's order into the renderer is not a fix: it would make one message
match and say nothing about the next enum.

The other half of this divergence — the clause echoing the value the member held, which
Bedrock's recorded clause does not — is FIXED: rule 5 of `src/service/validation.rs` now
states the position alone for a clause whose requirement is an enum's value set, pinned by
`tests/all/validation_message_wording.rs::an_enum_clause_states_the_admitted_set_and_not_the_value`
over enum-constrained members of three operations. That test asserts the set member by
member rather than as one ordered list, precisely because the order is this open question;
a fixer closing it should tighten those rows to assert the whole clause.

## Evidence

The one place the set is rendered, `src/service/validation.rs`:

```
/// An enumerated value set, as a requirement reads.
fn enum_requirement<S: AsRef<str>>(values: &[S]) -> String {
    let values: Vec<&str> = values.iter().map(AsRef::as_ref).collect();
    format!("Member must satisfy enum value set: [{}]", values.join(", "))
}
```

`values` is the `EnumTrait` the generated member schema carries, in the order codegen
captured; `src/ops/types/shared_types.rs` spells the same set for this enum as
`DATA_RETENTION_MODE_VALUES = ["aws_review", "default", "inherit", "none",
"provider_data_share"]`.

The message Bedrock is recorded answering for a `PutAccountDataRetention` whose `mode` is
outside the enum:

```
1 validation error detected: Value at 'mode' failed to satisfy constraint: Member must satisfy enum value set: [default, provider_data_share, none, aws_review, inherit]
```

## Steps to Reproduce

1. Boot the bedrock emulator.
2. `PUT /data-retention` with `{"mode": "not-a-mode"}`.
3. Observe HTTP 400 `ValidationException` whose message states
   `Value at 'mode' failed to satisfy constraint: Member must satisfy enum value set:
   [aws_review, default, inherit, none, provider_data_share]` — the right status, code,
   position and requirement, with the set in another order than the recorded one above.
4. The same difference shows for any enum-constrained member of any operation of this
   crate, because one renderer words them all.
