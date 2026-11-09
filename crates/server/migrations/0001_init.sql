-- WikiCraft initial schema

CREATE TABLE workspaces (
    id          uuid PRIMARY KEY,
    name        text NOT NULL,
    settings    jsonb NOT NULL DEFAULT '{}',
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id             uuid PRIMARY KEY,
    workspace_id   uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    email          text,
    name           text NOT NULL,
    password_hash  text,
    -- owner | admin | member | guest
    role           text NOT NULL DEFAULT 'member',
    -- active | deactivated | placeholder (imported author without account)
    status         text NOT NULL DEFAULT 'active',
    avatar_url     text,
    locale         text,
    timezone       text,
    external_ref   text,
    created_at     timestamptz NOT NULL DEFAULT now(),
    last_login_at  timestamptz
);
CREATE UNIQUE INDEX users_email ON users (workspace_id, lower(email)) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX users_external_ref ON users (workspace_id, external_ref) WHERE external_ref IS NOT NULL;

CREATE TABLE sessions (
    token_hash    bytea PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL,
    last_seen_at  timestamptz NOT NULL DEFAULT now(),
    user_agent    text,
    ip            text
);
CREATE INDEX sessions_user ON sessions (user_id);

CREATE TABLE api_tokens (
    id            uuid PRIMARY KEY,
    user_id       uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name          text NOT NULL,
    token_hash    bytea NOT NULL UNIQUE,
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz,
    last_used_at  timestamptz
);

CREATE TABLE groups (
    id            uuid PRIMARY KEY,
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name          text NOT NULL,
    description   text,
    external_ref  text,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX groups_name ON groups (workspace_id, lower(name));

CREATE TABLE group_members (
    group_id  uuid NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, user_id)
);
CREATE INDEX group_members_user ON group_members (user_id);

CREATE TABLE spaces (
    id                uuid PRIMARY KEY,
    workspace_id      uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    key               text NOT NULL,
    name              text NOT NULL,
    description       text,
    icon              text,
    home_page_id      uuid,
    -- role every workspace member (non-guest) gets: none | viewer | commenter | editor | admin
    default_role      text NOT NULL DEFAULT 'editor',
    anonymous_access  boolean NOT NULL DEFAULT false,
    personal_owner_id uuid REFERENCES users(id) ON DELETE SET NULL,
    created_by        uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    archived_at       timestamptz,
    source_ref        text
);
CREATE UNIQUE INDEX spaces_key ON spaces (workspace_id, lower(key));

-- explicit grants: subject_type user|group, role viewer|commenter|editor|admin
CREATE TABLE space_permissions (
    space_id      uuid NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    subject_type  text NOT NULL,
    subject_id    uuid NOT NULL,
    role          text NOT NULL,
    PRIMARY KEY (space_id, subject_type, subject_id)
);

CREATE TABLE pages (
    id            uuid PRIMARY KEY,
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    space_id      uuid NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    parent_id     uuid REFERENCES pages(id) ON DELETE SET NULL,
    -- page | folder | blog | whiteboard | database
    kind          text NOT NULL DEFAULT 'page',
    title         text NOT NULL,
    icon          text,
    position      double precision NOT NULL DEFAULT 0,
    content       jsonb NOT NULL DEFAULT '{"type":"doc","content":[{"type":"paragraph"}]}',
    text_content  text NOT NULL DEFAULT '',
    ydoc          bytea,
    version       integer NOT NULL DEFAULT 1,
    -- current | archived
    status        text NOT NULL DEFAULT 'current',
    page_status   text,
    full_width    boolean NOT NULL DEFAULT false,
    created_by    uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_by    uuid REFERENCES users(id) ON DELETE SET NULL,
    updated_at    timestamptz NOT NULL DEFAULT now(),
    deleted_at    timestamptz,
    deleted_by    uuid REFERENCES users(id) ON DELETE SET NULL,
    source_ref    text
);
CREATE INDEX pages_tree ON pages (space_id, parent_id, position) WHERE deleted_at IS NULL;
CREATE INDEX pages_updated ON pages (workspace_id, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX pages_source_ref ON pages (workspace_id, source_ref) WHERE source_ref IS NOT NULL;

CREATE TABLE page_versions (
    id          uuid PRIMARY KEY,
    page_id     uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    version     integer NOT NULL,
    title       text NOT NULL,
    content     jsonb NOT NULL,
    created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    message     text,
    source_ref  text,
    UNIQUE (page_id, version)
);

-- view restrictions are inherited by descendants; edit restrictions apply to the page itself
CREATE TABLE page_restrictions (
    page_id       uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    kind          text NOT NULL,
    subject_type  text NOT NULL,
    subject_id    uuid NOT NULL,
    PRIMARY KEY (page_id, kind, subject_type, subject_id)
);

CREATE TABLE blobs (
    key         text PRIMARY KEY,  -- sha256 hex
    size        bigint NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE attachments (
    id            uuid PRIMARY KEY,
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    page_id       uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    filename      text NOT NULL,
    media_type    text NOT NULL,
    size          bigint NOT NULL,
    blob_key      text REFERENCES blobs(key),
    version       integer NOT NULL DEFAULT 1,
    text_content  text,
    created_by    uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    deleted_at    timestamptz,
    source_ref    text
);
CREATE INDEX attachments_page ON attachments (page_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX attachments_name ON attachments (page_id, filename) WHERE deleted_at IS NULL;

CREATE TABLE attachment_versions (
    id             uuid PRIMARY KEY,
    attachment_id  uuid NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
    version        integer NOT NULL,
    blob_key       text REFERENCES blobs(key),
    size           bigint NOT NULL,
    media_type     text NOT NULL,
    created_by     uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    comment        text,
    UNIQUE (attachment_id, version)
);

CREATE TABLE comments (
    id            uuid PRIMARY KEY,
    page_id       uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    parent_id     uuid REFERENCES comments(id) ON DELETE CASCADE,
    author_id     uuid REFERENCES users(id) ON DELETE SET NULL,
    content       jsonb NOT NULL,
    text_content  text NOT NULL DEFAULT '',
    -- inline comments: {"text": "<selected text>"}; the anchor lives in the page as a `comment` mark
    inline        jsonb,
    resolved_at   timestamptz,
    resolved_by   uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    deleted_at    timestamptz,
    source_ref    text
);
CREATE INDEX comments_page ON comments (page_id, created_at);

CREATE TABLE reactions (
    target_type  text NOT NULL,
    target_id    uuid NOT NULL,
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    emoji        text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (target_type, target_id, user_id, emoji)
);

CREATE TABLE labels (
    page_id  uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    name     text NOT NULL,
    PRIMARY KEY (page_id, name)
);
CREATE INDEX labels_name ON labels (name);

CREATE TABLE favorites (
    user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    page_id     uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, page_id)
);

CREATE TABLE recent_views (
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    page_id    uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    viewed_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, page_id)
);
CREATE INDEX recent_views_user ON recent_views (user_id, viewed_at DESC);

CREATE TABLE watches (
    user_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    page_id   uuid NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, page_id)
);

CREATE TABLE notifications (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind        text NOT NULL,
    page_id     uuid REFERENCES pages(id) ON DELETE CASCADE,
    actor_id    uuid REFERENCES users(id) ON DELETE SET NULL,
    data        jsonb NOT NULL DEFAULT '{}',
    created_at  timestamptz NOT NULL DEFAULT now(),
    read_at     timestamptz
);
CREATE INDEX notifications_user ON notifications (user_id, created_at DESC);

CREATE TABLE audit_events (
    id            bigserial PRIMARY KEY,
    workspace_id  uuid NOT NULL,
    actor_id      uuid,
    action        text NOT NULL,
    target_type   text,
    target_id     uuid,
    data          jsonb NOT NULL DEFAULT '{}',
    ip            text,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_events_ws ON audit_events (workspace_id, created_at DESC);

CREATE TABLE jobs (
    id            uuid PRIMARY KEY,
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind          text NOT NULL,
    -- queued | running | succeeded | failed
    status        text NOT NULL DEFAULT 'queued',
    progress      jsonb NOT NULL DEFAULT '{}',
    result        jsonb,
    error         text,
    created_by    uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    started_at    timestamptz,
    finished_at   timestamptz
);

-- maps ids from external systems (e.g. Confluence) to WikiCraft ids; used for re-imports,
-- cross-import link resolution and redirects of old URLs
CREATE TABLE import_mappings (
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    source        text NOT NULL,   -- e.g. 'confluence'
    source_type   text NOT NULL,   -- page | attachment | comment | space | user
    source_id     text NOT NULL,
    target_id     uuid NOT NULL,
    job_id        uuid,
    PRIMARY KEY (workspace_id, source, source_type, source_id)
);
