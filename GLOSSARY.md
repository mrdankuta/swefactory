# Glossary (rename terms for this domain)

One term per line: **Term**: definition. Use these exact terms in spec/ticket
issues and in code (module names, field names, UI copy).

**AgentRunner**: the trait that runs one agent provider (builds its command, parses its stream events, owns its session storage). One implementation per agent; pi-headless is the first, Claude/Codex later.
**session identity**: the agent-session id, captured session file, and token usage threaded onto the attempt ledger as join keys. The attempt stays authoritative; the session annotates it.
**verdict tag**: the typed `<verdict>{...}</verdict>` payload in reviewer output, schema-validated with last-match-wins. Replaces prose-verdict scanning.
