Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 50acf5c50341569d447ddf04c6c32ab03f586e64

# The partner-input replay branch answers 201 with a DELETED partner input, the liveness rule the same handler applies eleven lines below

_Filed: 2026-10-29T03:51:33Z_
_Operation: CreatePartnerInput | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `50acf5c50341569d447ddf04c6c32ab03f586e64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteInput` is a tombstone delete: the input moves to `DELETING` and the clock settles it into the terminal `DELETED`, and the record stays in `self.inputs` so every reader keeps reporting it there. The crate's rule for that state is explicit — a mutation resolves an input id through `live_input` and refuses a dead one — and `CreatePartnerInput` applies it to the PRIMARY input it is given.

Its idempotency replay branch does not. The branch resolves the token through `input_for_token` (`self.inputs.get(id)`, no state filter) and then filters only on `partner_of`, so a tombstoned partner still matches. `input_tokens` is also the one idempotency map in this crate that is never `forget`-ten: the inputs arm of `ComAmazonawsMedialiveMedialiveState::settle` just settles each record, while the channel and network arms immediately beside it drop the tokens of anything that reached `DELETED`, with a comment stating the rule.

So a `CreatePartnerInput` retried with the `RequestId` of a partner that has since been deleted answers 201 with that DELETED input — an id `UpdateInput` refuses with 404 "already deleted", that reports no destinations any more, and that nothing can be done with — and creates and stores nothing. The same branch also fires when the PRIMARY input has since been deleted, because it runs before the primary-liveness check at line 200, so the call reports a partner of an input the account no longer holds instead of the refusal the handler's own next statement (and the test `a_partner_of_an_unusable_input_is_refused`) exists for.

## Evidence

The crate's stated rule for a tombstoned input — services/service-com-amazonaws-medialive-medialive/src/state.rs:3517-3530:
```
    /// The input `id` names while it is still LIVE: a record whose delete this
    /// service has not accepted.
    ///
    /// The lookup a MUTATION of an input goes through. A delete never drops the
    /// record — it keeps it in `DELETING` and then in the terminal `DELETED`, so
    /// a reader goes on reporting it — but that record is no longer a resource a
    /// caller may change, so a mutation resolves the id through here and refuses
    /// a dead one with its own modelled not-found error. ...
    pub fn live_input(&self, id: &str) -> Option<&InputRecord> {
        self.inputs
            .get(id)
            .filter(|input| !input.is_delete_accepted())
    }
```

The token lookup has no such filter — src/state.rs:3432-3443:
```
    /// Record `token` as having created `input_id`, under the same bounded,
    /// evicting discipline as the channel tokens.
    pub fn record_input_idempotency_token(&mut self, token: String, input_id: &str) {
        self.input_tokens
            .record(token, input_id, MAX_INPUT_IDEMPOTENCY_TOKENS);
    }

    /// The input a stored `RequestId` created, if that input still exists.
    pub fn input_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.input_tokens.created(request_id)?;
        self.inputs.get(id).map(|input| input.id.as_str())
    }
```
— and `created` is documented as answering "whether or not that resource still exists" (src/state.rs:1785-1788).

The inputs arm of `settle` forgets nothing, between two arms that do — src/state.rs:2459-2485:
```
        // A token never outlives the channel it minted: once that channel has
        // reached the terminal `DELETED` state its idempotency window is over, so
        // a retried create mints a new channel rather than answering with a
        // deleted one.
        for id in just_deleted {
            self.channel_tokens.forget(&id);
        }
        for input in self.inputs.values_mut() {
            input.settle(now);
        }
        ...
        for id in deleted_networks {
            self.network_tokens.forget(&id);
        }
```
(`grep -n 'input_tokens' src` yields only the field declaration, `record` and `created` — no `forget`.)

The replay branch, and the liveness check eleven lines below it that it bypasses — src/ops/create_partner_input.rs:173-209:
```
        if let Some(existing) = input
            .request_id
            .as_deref()
            .and_then(|token| state.input_for_token(token))
            .filter(|existing| {
                state
                    .inputs
                    .get(*existing)
                    .is_some_and(|stored| stored.partner_of.as_deref() == Some(&input.input_id))
            })
            .map(str::to_string)
        {
            let created = state.input_description(&existing).ok_or_else(unavailable)?;
            return Ok(CreatePartnerInputOutput { input: Some(created) });
        }

        // The partner is created FOR the input the path named, so that input must
        // be one this account still holds and may still change: a delete this
        // service has accepted leaves the record readable, but the id no longer
        // names an input a caller may give a partner. ...
        let Some(primary) = state.live_input(&input.input_id).cloned() else {
            let message = if state.inputs.contains_key(&input.input_id) {
                crate::state::already_deleted_message(&crate::state::deleted_input_subject(
                    &input.input_id,
                ))
            } else {
                crate::state::not_found_message("Input", &input.input_id)
            };
            return Err(bad_request(&message));
        };
```

The replayed id is unusable — src/ops/update_input.rs:202-220:
```
        if state.live_input(&input.input_id).is_none() {
            // Absent and already deleted are ONE refusal: ... the id names nothing a caller may
            // still change, so the mutation is answered with this operation's
            // modelled `NotFoundException` (404) and nothing is written.
```

Test coverage: the partner idempotency table covers only live-partner rows and never re-presents a token after a delete — tests/all/partner_input.rs:344-355 (`a_repeated_request_id_creates_one_partner_input`, rows "same token" / "different tokens" / "no token"); tests/all/partner_input.rs:546 (`a_partner_of_an_unusable_input_is_refused`) pins the primary-liveness refusal the replay branch runs ahead of; and the same post-delete rule IS pinned for a resource that implements it — tests/all/signal_map_deletion.rs (`a_deleted_signal_map_is_not_resurrected_by_its_idempotency_token`).

## Steps to Reproduce

1. POST /prod/inputs with {"name":"primary","type":"AWS_CDI","destinations":[{"streamName":"cdi-a"}]} -> 201; note the input id P.
2. POST /prod/inputs/{P}/partners with {"requestId":"token-a"} -> 201; note the partner input id Q.
3. DELETE /prod/inputs/{Q} -> Q reports state DELETING; wait for the transient span and confirm GET /prod/inputs/{Q} reports state DELETED (the record is kept).
4. POST /prod/inputs/{P}/partners again with {"requestId":"token-a"} -> the emulator answers 201 carrying input Q with "state":"DELETED" and creates nothing; GET /prod/inputs/{P} shows no new partner in inputPartnerIds. Expected, per src/state.rs:3517-3530 and the forget rule src/state.rs:2459-2465, a freshly minted partner input.
5. PUT /prod/inputs/{Q} with {"name":"anything"} -> 404 "already deleted" (src/ops/update_input.rs:202-220), so the 201 above named an input the caller can neither use nor change.
6. Second path: with token "token-b", create a partner R of P, then DELETE /prod/inputs/{P} and wait for DELETED. POST /prod/inputs/{P}/partners with {"requestId":"token-b"} -> 201 carrying R, instead of the "already deleted" refusal src/ops/create_partner_input.rs:200-209 answers for the same request without a token.
