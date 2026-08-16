# coven-agents

`coven-agents` is an experimental, provider-neutral Rust run loop for
OpenCoven. It supplies the orchestration primitives that model adapters,
applications, and familiar runtimes can share:

- bounded model/tool loops
- agent handoffs
- blocking input and output guardrails
- pluggable session journals
- metadata-only lifecycle observation

Input guardrails apply to the starting agent, and output guardrails apply to the
agent that produces the final output. A `SessionStore` must serialize writers
for each session id or implement optimistic concurrency control.

The crate deliberately does not include an OpenAI client, a daemon command,
MCP, sandbox execution, voice, or realtime transport. Those are adapters and
application concerns. Keeping this crate as a workspace leaf also allows it to
move into its own repository if the API stabilizes.

The implementation was derived from public behavioral documentation and
OpenCoven's existing runtime requirements, not from another SDK's source code.
See the design document under `docs/superpowers/specs/`.
