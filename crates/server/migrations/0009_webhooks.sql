-- Outgoing webhooks (admin-configured), delivered from an outbox with retries.
CREATE TABLE webhooks (
    id               uuid PRIMARY KEY,
    workspace_id     uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name             text NOT NULL DEFAULT '',
    url              text NOT NULL,
    secret           text NOT NULL,
    -- event names; empty = all events
    events           text[] NOT NULL DEFAULT '{}',
    space_id         uuid REFERENCES spaces(id) ON DELETE CASCADE,
    active           boolean NOT NULL DEFAULT true,
    created_by       uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    failure_count    integer NOT NULL DEFAULT 0
);

CREATE TABLE webhook_deliveries (
    id               uuid PRIMARY KEY,
    webhook_id       uuid NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
    event            text NOT NULL,
    payload          jsonb NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    next_attempt_at  timestamptz NOT NULL DEFAULT now(),
    attempts         integer NOT NULL DEFAULT 0,
    delivered_at     timestamptz,
    failed_at        timestamptz,
    response_status  integer,
    last_error       text,
    duration_ms      integer
);
CREATE INDEX webhook_deliveries_pending ON webhook_deliveries (next_attempt_at) WHERE delivered_at IS NULL AND failed_at IS NULL;
CREATE INDEX webhook_deliveries_hook ON webhook_deliveries (webhook_id, created_at DESC);
