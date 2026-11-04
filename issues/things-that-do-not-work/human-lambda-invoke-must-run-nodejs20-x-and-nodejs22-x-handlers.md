# Lambda Invoke must run nodejs20.x and nodejs22.x handlers

Lambda Invoke (RequestResponse) for a function with Runtime nodejs20.x or nodejs22.x must
run the handler from the function's stored .zip deployment package and return what the
handler returned. Today it answers 501 NotImplemented.

- Handler `index.handler` works as CommonJS (`exports.handler`) and as an ES module
  (`export const handler`), in async and callback styles, and receives the event JSON and a
  context object.
- A returned value comes back as the Invoke payload.
- A thrown error comes back the way Lambda reports an unhandled function error, with the
  errorType / errorMessage / trace document as the payload. With LogType=Tail the response
  carries the last 4 KB of console output.
- The function's Timeout and MemorySize are enforced; a run that exceeds either ends with
  Lambda's error for that case.
- Handler code cannot reach the host filesystem, network, processes or native modules;
  `require` resolves only files in the package and the node core modules the emulator provides.

Repro: Lambda CreateFunction (Runtime=nodejs20.x, Handler=index.handler, zip containing
`exports.handler = async (e) => ({ ok: true, echo: e })`), then Invoke with {"a":1}.
Expected payload {"ok":true,"echo":{"a":1}}; today 501.
