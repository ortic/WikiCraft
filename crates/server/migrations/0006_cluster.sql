-- Short-lived auth state (OIDC/SAML login flows) shared by all replicas
CREATE TABLE auth_states (
    state       text PRIMARY KEY,
    data        jsonb NOT NULL,
    expires_at  timestamptz NOT NULL
);

-- Failed-login throttle shared by all replicas
CREATE TABLE login_attempts (
    email         text PRIMARY KEY,
    failures      integer NOT NULL,
    window_start  timestamptz NOT NULL
);

-- Collaboration relay between replicas (WIKICRAFT_CLUSTER=postgres); rows are short-lived
CREATE UNLOGGED TABLE collab_updates (
    id          bigserial PRIMARY KEY,
    page_id     uuid NOT NULL,
    origin      uuid NOT NULL,
    kind        smallint NOT NULL,   -- 0 = document update, 1 = awareness
    payload     bytea NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX collab_updates_created ON collab_updates (created_at);

-- Postgres full-text search backend (WIKICRAFT_SEARCH=postgres)
ALTER TABLE pages ADD COLUMN search_vec tsvector;
CREATE INDEX pages_search ON pages USING gin (search_vec);

-- Job queue: input location, worker claim + heartbeat
ALTER TABLE jobs ADD COLUMN input jsonb;
ALTER TABLE jobs ADD COLUMN worker text;
ALTER TABLE jobs ADD COLUMN heartbeat_at timestamptz;
CREATE INDEX jobs_queue ON jobs (status, created_at);
