# Context, lexical memory, and shared state

Run this captured local agent with a trusted host configuration granting the
`demo` memory scope. Both the agent request and operator grant are required.
The CLI invocation is documented in the repository agent guide.

`ctx.context.add({role, content})` returns `{messages, archived}` and
`ctx.context.messages()` returns active messages. Context belongs to the run.
The default limits are 128 messages and 256 KiB of serialized message bytes;
compaction archives the oldest eligible messages, preserving all system messages
and the most recent 16 messages. Oversized protected context is rejected and its
mutation is rolled back. Compaction decisions and removed messages remain in
SQLite. This is deterministic removal, without model-generated summaries.

`remember(text, {scope, metadata})` stores lexical facts; `recall(query,
{scope, limit})` searches SQLite FTS5 (default 10, maximum 100), returning
`{id, text, scope, metadata}` matches. It uses lexical terms, not embeddings.
Recorded retrieval responses are reused during replay even if shared facts
change. Scope defaults to `default` and still requires an explicit grant.

`state.get(key, {scope})` returns `{version, value}` or `null`.
`state.set(key, value, {scope})` returns `{updated:true, version, value}`.
`state.compareAndSet(key, expectedVersion, value, {scope})` returns the same
success shape or `{updated:false, version}` on conflict. Missing keys have
version 0. Shared mutations, context compaction, and their operation outcomes
commit in one SQLite transaction, so a restart cannot repeat a committed write.
State is separate from conversation memory, execution history, and model caches.
