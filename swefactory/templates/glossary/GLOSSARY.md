# Glossary (project vocabulary — use these exact terms in spec/ticket issues and in code)

One term per line: **Term**: definition. Fill this in during `/plan`
(domain-modeling creates entries lazily as terms get resolved).

- **Factory**: the method (spec → freeze → tickets → isolated builds → review → release train → human-owned land).
- **Spec**: a product spec issue; authoritative at the `SPEC-APPROVED` freeze.
- **Ticket**: a single unit of implementation work (one worktree + branch).
- **Release train**: an `integration/*` branch accumulating accepted tickets; only a human lands it.
