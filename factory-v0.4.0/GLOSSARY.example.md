# Domain glossary (authoritative vocabulary — use these terms in specs, tickets, code)

## Tenancy & identity
- **Firm**: the tenant. All firm-scoped data carries `firm_id`. Resolved by `SetTenant` plug (subdomain or custom domain lookup).
- **Client organization (client org)**: a firm's customer company/household. Owns engagements, documents, tasks, conversations.
- **User**: global identity (one row per email). Linked via memberships.
- **Membership**: user ↔ firm/org join with a role.
- **Role**: `firm-admin` (full firm control), `firm-staff` (works engagements), `client-admin` (manages one org), `client-member` (views org data). Plus `platform_admin` boolean for the Admin namespace.
- **Invitation**: signed single-use token (7-day expiry) tying an email to an org + role; membership created after OAuth/login.

## Work & compliance
- **Service definition**: firm-configured type of professional work (Bookkeeping, Payroll, GST/HST, SK PST, T1, T2, T3…).
- **Engagement**: a client org's active service instance (frequency, account manager, software tracking).
- **Rule version**: versioned deadline rule for a service/jurisdiction (draft → active; editing an active rule auto-clones a draft; activation runs impact preview + batched recalculation).
- **Deliverable**: period-specific compliance obligation generated from an active rule (e.g. "2026-08 SK PST return").
- **Deliverable schedule**: the generated series of deliverables for an engagement.
- **Task**: ad-hoc or deliverable-linked work item (title, status, priority, due date, assignee). Legacy `recurring-task` rows are read-only; new dues flow into deliverable schedules.
- **Disposition** (legacy migration): per-row outcome when migrating legacy recurring work (migrated / skipped / manual).

## Content & collaboration
- **Document / Folder**: virtual folder tree (adjacency list) over flat R2 keys (`firm-{id}/org-{id}/{uuid}.{ext}`). Downloads via auth-gated presigned URLs.
- **Signature envelope**: e-signature request tracked in `document_signatures` (draft/sent/viewed/signed/declined/completed), via self-hosted OpenSign/Documenso over HTTPS + webhook.
- **Conversation / Message**: threaded categorized messaging with @mentions, attachments, stars; LiveView streams + PubSub; presence is conversation-scoped (clients) and firm-wide (staff only).
- **Credential (vault)**: AES-GCM (`cloak_ecto`) encrypted client secret; only assigned account manager + firm admins can decrypt; every decryption is access-logged.
- **Tax account**: jurisdiction-aware encrypted client tax identifier used by the compliance engine.
- **Calendar event**: per-org event (deadline/meeting/filing) with reminders via Oban; firm-wide aggregated view for staff.
- **Notification**: in-app + email (Brevo/Swoosh, Oban-batched; @mentions fast-path, rest digested). Obligation aliases retired — notify on deliverables.
- **Subscription**: Stripe tier by client-org count (Starter 1–5 / Professional 6–25 / Enterprise 25+). Not yet implemented.
