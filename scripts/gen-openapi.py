#!/usr/bin/env python3
"""Generates crates/server/openapi.yaml (OpenAPI 3.1) from the curated table below.
Run after adding routes; `cargo test -p wikicraft --lib openapi` fails when a route is missing."""
import yaml, pathlib

S = lambda name: {"$ref": f"#/components/schemas/{name}"}
arr = lambda x: {"type": "array", "items": x}
obj = lambda props, req=None: {"type": "object", "properties": props, **({"required": req} if req else {})}
uuid = {"type": "string", "format": "uuid"}
dt = {"type": "string", "format": "date-time"}
txt = {"type": "string"}
num = {"type": "integer"}
boolean = {"type": "boolean"}
anyobj = {"type": "object", "additionalProperties": True}

schemas = {
    "Error": obj({"error": txt, "message": txt}),
    "User": obj({"id": uuid, "workspaceId": uuid, "email": {"type": ["string", "null"]}, "name": txt, "role": {"enum": ["owner", "admin", "member", "guest"]}, "status": {"enum": ["active", "invited", "deactivated", "placeholder"]}, "avatarUrl": {"type": ["string", "null"]}}),
    "Space": obj({"id": uuid, "key": txt, "name": txt, "description": {"type": ["string", "null"]}, "icon": {"type": ["string", "null"], "description": "symbol[:color] from the WikiCraft icon set"}, "homePageId": uuid, "category": {"type": ["string", "null"]}, "defaultRole": {"enum": ["none", "viewer", "commenter", "editor", "admin"]}, "anonymousAccess": boolean, "role": txt, "starred": boolean, "watching": boolean}),
    "PmNode": {"type": "object", "description": "ProseMirror / Tiptap JSON node (type, attrs, content, text, marks)", "additionalProperties": True},
    "PageAccess": obj({"view": boolean, "comment": boolean, "edit": boolean, "admin": boolean}),
    "Page": obj({"id": uuid, "spaceId": uuid, "spaceKey": txt, "spaceName": txt, "parentId": {"type": ["string", "null"], "format": "uuid"}, "kind": {"enum": ["page", "folder", "blog"]}, "title": txt, "content": S("PmNode"), "version": num, "labels": arr(txt), "restricted": boolean, "createdAt": dt, "updatedAt": dt, "updatedByName": {"type": ["string", "null"]}, "breadcrumbs": arr(obj({"id": uuid, "title": txt})), "access": S("PageAccess")}),
    "PageListItem": obj({"id": uuid, "title": txt, "spaceId": uuid, "spaceKey": txt, "spaceName": txt, "updatedAt": dt, "updatedByName": {"type": ["string", "null"]}}),
    "NewPage": obj({"spaceKey": txt, "parentId": uuid, "title": txt, "draft": {"type": "boolean", "description": "an unpublished draft only you see until you publish it"}, "kind": {"enum": ["page", "folder", "blog"]}, "content": S("PmNode"), "markdown": {"type": "string", "description": "alternative to content"}, "templateId": txt, "variables": {"type": "object", "additionalProperties": {"type": "string"}, "description": "answers for the template's {{variables}} (by name); a variable without an answer becomes a hint to fill in, an empty answer removes it"}}, ["spaceKey"]),
    "Comment": obj({"id": uuid, "pageId": uuid, "parentId": {"type": ["string", "null"], "format": "uuid"}, "authorId": uuid, "authorName": txt, "content": S("PmNode"), "inline": {"type": ["object", "null"]}, "resolvedAt": {"type": ["string", "null"], "format": "date-time"}, "createdAt": dt, "likeCount": num, "liked": boolean}),
    "Attachment": obj({"id": uuid, "pageId": uuid, "filename": txt, "mediaType": txt, "size": num, "version": num, "createdByName": {"type": ["string", "null"]}, "createdAt": dt}),
    "SearchHit": obj({"pageId": uuid, "title": txt, "snippet": {"type": "string", "description": "HTML with <b> highlights"}, "spaceKey": txt, "spaceName": txt, "score": {"type": "number"}}),
    "Job": obj({"id": uuid, "kind": txt, "status": {"enum": ["queued", "running", "succeeded", "failed"]}, "progress": anyobj, "result": anyobj, "error": {"type": ["string", "null"]}, "createdAt": dt, "finishedAt": {"type": ["string", "null"], "format": "date-time"}}),
    "ShareLink": obj({"id": uuid, "token": txt, "includeChildren": boolean, "expiresAt": {"type": ["string", "null"], "format": "date-time"}, "views": num}),
    "Webhook": obj({"id": uuid, "name": txt, "url": txt, "events": arr(txt), "format": txt, "spaceKey": {"type": ["string", "null"]}, "active": boolean, "failureCount": num}),
    "WebhookEvent": obj({"event": txt, "timestamp": dt, "webhookId": uuid, "deliveryId": uuid, "data": anyobj}),
    "Task": obj({"pageId": uuid, "idx": num, "text": txt, "done": boolean, "due": {"type": ["string", "null"], "format": "date"}, "pageTitle": txt, "spaceKey": txt}),
    "Restriction": obj({"kind": {"enum": ["view", "edit"]}, "subjectType": {"enum": ["user", "group"]}, "subjectId": uuid}),
    "Group": obj({"id": uuid, "name": txt, "memberCount": num}),
    "Notification": obj({"id": uuid, "kind": txt, "pageId": uuid, "pageTitle": txt, "actorName": txt, "data": anyobj, "createdAt": dt, "readAt": {"type": ["string", "null"], "format": "date-time"}}),
}

def body(schema, ctype="application/json"):
    return {"required": True, "content": {ctype: {"schema": schema}}}

def multipart(fields):
    return body(obj({**fields, "file": {"type": "string", "format": "binary"}}, ["file"]), "multipart/form-data")

def resp(schema=None, code="200", desc="OK"):
    r = {"description": desc}
    if schema is not None:
        r["content"] = {"application/json": {"schema": schema}}
    return {code: r}

NO = {"204": {"description": "Done"}}
# (path, method, tag, summary, extra) — extra: request body / responses / public / params
R = []
def op(path, method, tag, summary, req=None, res=None, public=False, desc=None, query=None):
    R.append((path, method, tag, summary, req, res, public, desc, query))

q = lambda name, d, schema=txt: {"name": name, "in": "query", "required": False, "description": d, "schema": schema}

# auth & account
op("/setup", "get", "Auth", "Whether first-run setup is needed", res=resp(obj({"needsSetup": boolean})), public=True)
op("/setup", "post", "Auth", "First-run setup: create the workspace and its owner", req=body(obj({"workspaceName": txt, "name": txt, "email": txt, "password": txt})), public=True)
op("/auth/login", "post", "Auth", "Sign in with e-mail (or directory username) and password", req=body(obj({"email": txt, "password": txt})), res=resp(obj({"ok": boolean, "mfaRequired": boolean, "mfaToken": txt}), desc="Session cookie set, or a two-factor challenge"), public=True)
op("/auth/mfa", "post", "Auth", "Complete a sign-in with a TOTP or recovery code", req=body(obj({"mfaToken": txt, "code": txt})), public=True)
op("/auth/logout", "post", "Auth", "Sign out (deletes the session)", res=NO)
op("/auth/providers", "get", "Auth", "Configured sign-in providers (SSO buttons, LDAP)", public=True)
op("/auth/forgot", "post", "Auth", "Request a password reset e-mail (always 204)", req=body(obj({"email": txt})), res=NO, public=True)
op("/auth/reset", "post", "Auth", "Set a new password with a reset token (signs in)", req=body(obj({"token": txt, "password": txt})), public=True)
op("/auth/invite", "get", "Auth", "Invitation details for a token", public=True, query=[{"name": "token", "in": "query", "required": True, "schema": txt}])
op("/auth/accept-invite", "post", "Auth", "Accept an invitation (sets name and password, signs in)", req=body(obj({"token": txt, "name": txt, "password": txt})), public=True)
op("/auth/signup", "get", "Auth", "Whether sign-up is open and for which e-mail domains", res=resp(obj({"enabled": boolean, "domains": arr(txt)})), public=True)
op("/auth/signup", "post", "Auth", "Ask for an account (allowed domains only): mails a confirmation link; same answer for every address", req=body(obj({"email": txt}, ["email"])), res=resp(None, "204", "Requested"), public=True)
op("/admin/signup", "get", "Admin", "Sign-up domains", res=resp(obj({"domains": arr(txt), "mailConfigured": boolean})))
op("/admin/signup", "put", "Admin", "Set the sign-up domains (empty = invitation only)", req=body(obj({"domains": arr(txt)}, ["domains"])), res=resp(obj({"domains": arr(txt), "mailConfigured": boolean})))
ANNOUNCEMENT = obj({"text": txt, "level": {"enum": ["info", "warning", "success"]}, "id": txt})
op("/announcement", "get", "Admin", "The workspace announcement banner (null when there is none)", res=resp(ANNOUNCEMENT))
op("/admin/announcement", "put", "Admin", "Set the announcement banner (empty text removes it)", req=body(obj({"text": txt, "level": {"enum": ["info", "warning", "success"]}}, ["text"])), res=resp(ANNOUNCEMENT))
op("/auth/sso/{id}/login", "get", "Auth", "Start SSO login (redirect to the identity provider)", public=True, query=[q("next", "path to return to")])
op("/auth/sso/{id}/callback", "get", "Auth", "OIDC / GitHub callback", public=True)
op("/auth/sso/{id}/acs", "post", "Auth", "SAML assertion consumer service (build feature `saml`)", public=True)
op("/auth/sso/{id}/metadata", "get", "Auth", "SAML service-provider metadata (build feature `saml`)", public=True)
op("/auth/oidc", "get", "Auth", "Legacy single-OIDC provider info", public=True)
op("/auth/oidc/login", "get", "Auth", "Legacy single-OIDC login", public=True)
op("/auth/oidc/callback", "get", "Auth", "Legacy single-OIDC callback", public=True)
op("/me", "get", "Account", "Current user and workspace", res=resp(obj({"user": {"oneOf": [S("User"), {"type": "null"}]}, "workspace": obj({"id": uuid, "name": txt})})), public=True)
op("/me/tokens", "get", "Account", "Personal API tokens")
op("/me/tokens", "post", "Account", "Create a personal API token (shown once). A read scope allows no changes (GET, Markdown conversion, Ask the wiki; MCP read tools)", req=body(obj({"name": txt, "expiresInDays": num, "scope": {"enum": ["write", "read"]}})))
op("/me/tokens/{id}", "delete", "Account", "Delete a personal API token", res=NO)
op("/me/preferences", "get", "Account", "Notification e-mail preference")
op("/me/preferences", "put", "Account", "Set notification preferences (only the fields sent change)", req=body(obj({"emailNotifications": {"enum": ["all", "important", "none"]}, "emailDigest": {"enum": ["soon", "hourly", "daily"], "description": "how often notification e-mails go out"}, "muted": arr(txt)})))
op("/me/space", "get", "Spaces", "Your personal space (key or null)", res=resp(obj({"key": txt})))
op("/me/space", "post", "Spaces", "Create (or get) your private personal space", res=resp(S("Space"), "201", "Created"))
op("/me/sessions", "get", "Account", "Active sessions of the current user")
op("/me/sessions/{id}", "delete", "Account", "Sign out one session", res=NO)
op("/me/sessions/revoke-others", "post", "Account", "Sign out everywhere else")
op("/me/mfa", "get", "Account", "Two-factor status")
op("/me/mfa/setup", "post", "Account", "Start TOTP enrolment (secret, otpauth URI, QR SVG)")
op("/me/mfa/enable", "post", "Account", "Confirm TOTP enrolment; returns recovery codes", req=body(obj({"code": txt})))
op("/me/mfa/disable", "post", "Account", "Turn off two-factor (password or code)", req=body(obj({"password": txt, "code": txt})), res=NO)
op("/me/mfa/recovery-codes", "post", "Account", "Replace the recovery codes", req=body(obj({"code": txt})))
# users & groups
op("/people", "get", "People", "People directory (accounts, not imported authors); q searches name, title, department, location", query=[q("q", "search"), q("department", "only this department")], res=resp(obj({"people": arr(obj({"id": uuid, "name": txt, "jobTitle": {"type": ["string", "null"]}, "department": {"type": ["string", "null"]}, "location": {"type": ["string", "null"]}, "timezone": {"type": ["string", "null"]}, "avatarUrl": {"type": ["string", "null"]}})), "departments": arr(obj({"name": txt, "count": num}))})))
op("/people/{id}", "get", "People", "A profile: job, department, location, time zone, bio, personal space and the pages they edited lately (those you can see); e-mail for admins and themself")
op("/people/avatars", "get", "People", "Picture URLs by person id (people who uploaded one)", res=resp({"type": "object", "additionalProperties": txt}))
op("/me/avatar", "post", "People", "Upload a profile picture (PNG, JPEG, WebP or GIF up to 8 MB; cropped square, scaled to 256 px, stored as PNG)", req=multipart({"file": {"type": "string", "format": "binary"}}), res=resp(obj({"avatarUrl": txt})))
op("/me/avatar", "delete", "People", "Remove your profile picture", res=NO)
op("/users/{id}/avatar", "get", "People", "A profile picture (PNG)")
op("/me/profile", "put", "People", "Update your profile (fields left out stay; empty clears)", req=body(obj({"jobTitle": txt, "department": txt, "location": txt, "timezone": {"type": "string", "description": "IANA name, e.g. Europe/Zurich"}, "bio": txt})))
op("/users", "get", "Users", "People in the workspace", res=resp(arr(S("User"))), query=[q("q", "name/e-mail filter"), q("include_placeholders", "include imported authors", boolean)])
op("/users", "post", "Users", "Create a user with a password (admins)", req=body(obj({"email": txt, "name": txt, "password": txt, "role": txt})))
op("/users/invite", "post", "Users", "Invite someone by e-mail (admins)", req=body(obj({"email": txt, "name": txt, "role": txt})), res=resp(obj({"id": uuid, "inviteUrl": txt, "emailed": boolean}), "201", "Created"))
op("/users/{id}", "patch", "Users", "Update a user (self: name/password; admins: role/status)", req=body(obj({"name": txt, "role": txt, "status": txt, "password": txt})), res=NO)
op("/users/{id}/merge", "post", "Users", "Merge an imported placeholder user into an existing account: authorship, permissions, likes, watches and mentions move to it (admins)", req=body(obj({"intoUserId": uuid}, ["intoUserId"])), res=resp(obj({"mergedInto": uuid, "moved": anyobj})))
op("/users/{id}/reset-mfa", "post", "Users", "Reset a user's two-factor authentication (admins)", res=NO)
op("/groups", "get", "Users", "Groups", res=resp(arr(S("Group"))))
op("/groups", "post", "Users", "Create a group (admins)", req=body(obj({"name": txt})))
op("/groups/{id}", "delete", "Users", "Delete a group (admins)", res=NO)
op("/groups/{id}/members", "post", "Users", "Add a member", req=body(obj({"userId": uuid})), res=NO)
op("/groups/{id}/members/{user_id}", "delete", "Users", "Remove a member", res=NO)
# spaces
op("/spaces", "get", "Spaces", "Spaces visible to the caller", res=resp(arr(S("Space"))), public=True)
op("/spaces", "post", "Spaces", "Create a space", req=body(obj({"key": txt, "name": txt, "description": txt, "icon": txt, "defaultRole": txt}, ["key", "name"])), res=resp(S("Space"), "201", "Created"))
op("/spaces/{key}", "get", "Spaces", "A space", res=resp(S("Space")), public=True)
op("/spaces/{key}", "patch", "Spaces", "Update key (old key keeps working), name, description, icon, category, default role, anonymous access", req=body(anyobj))
op("/spaces/{key}", "delete", "Spaces", "Archive a space", res=NO)
op("/spaces/{key}/tree", "get", "Spaces", "Page tree (visible pages)", public=True)
op("/spaces/{key}/permissions", "get", "Spaces", "Space role grants")
op("/spaces/{key}/permissions", "put", "Spaces", "Replace space role grants", req=body(obj({"defaultRole": txt, "anonymousAccess": boolean, "grants": arr(obj({"subjectType": txt, "subjectId": uuid, "role": txt}))})), res=NO)
op("/spaces/{key}/trash/empty", "post", "Spaces", "Empty the space's trash: delete every trashed page you can see for good (space admins)", res=resp(obj({"purged": num, "kept": num})))
op("/spaces/{key}/trash", "get", "Spaces", "Trashed pages")
op("/spaces/{key}/blog", "get", "Spaces", "Blog posts", public=True)
op("/spaces/{key}/blog.atom", "get", "Spaces", "The space's blog as an Atom feed (latest 20 posts you can see; public spaces without signing in)", public=True)
op("/spaces/{key}/export", "post", "Import & export", "Export a space (HTML site, Markdown, backup) as a job", req=body(obj({"format": {"enum": ["html", "markdown", "backup"]}})), res=resp(obj({"jobId": uuid}), "202", "Queued"))
# pages
op("/pages", "post", "Pages", "Create a page", req=body(S("NewPage")), res=resp(obj({"id": uuid}), "201", "Created"))
op("/pages/{id}", "get", "Pages", "A page (content, labels, access)", res=resp(S("Page")), public=True)
op("/pages/{id}", "patch", "Pages", "Rename; set icon (symbol), cover (gradient:<preset> | attachment | https URL), status, full width", req=body(obj({"title": txt, "icon": txt, "cover": txt, "pageStatus": txt, "fullWidth": boolean})), res=NO)
op("/pages/{id}", "delete", "Pages", "Move a page and its sub-pages to the trash", res=NO)
op("/pages/{id}/content", "put", "Pages", "Replace the content (live editors get the change)", req=body(obj({"content": S("PmNode"), "markdown": txt, "message": txt, "minor": {"type": "boolean", "description": "a minor change: watchers aren't notified"}})))
op("/pages/{id}/move", "post", "Pages", "Move (parent, position, other space)", req=body(obj({"parentId": uuid, "spaceKey": txt, "position": {"type": "number"}})), res=NO)
op("/pages/{id}/copy", "post", "Pages", "Copy a page (optionally with sub-pages)", req=body(obj({"includeChildren": boolean, "parentId": uuid})))
op("/pages/{id}/publish", "post", "Pages", "Publish your unpublished draft: everyone who can see its place in the space sees it", res=NO)
op("/pages/{id}/publish-schedule", "put", "Pages", "Publish your draft automatically at a time (up to a year ahead); it publishes only if you can still edit it then", req=body(obj({"at": dt}, ["at"])), res=resp(obj({"publishAt": dt})))
op("/pages/{id}/publish-schedule", "delete", "Pages", "Cancel the scheduled publishing of your draft", res=NO)
op("/me/drafts", "get", "Account", "My unpublished drafts", res=resp(arr(obj({"id": uuid, "title": txt, "spaceKey": txt, "spaceName": txt, "updatedAt": dt}))))
op("/pages/{id}/viewers", "get", "Pages", "Who viewed the page and when (editors; only when an admin turned on the viewer list)", res=resp(arr(obj({"userId": uuid, "name": txt, "lastViewed": {"type": "string", "format": "date"}, "views": num}))))
op("/pages/{id}/purge", "post", "Pages", "Delete a trashed page for good, with its trashed sub-pages, history, comments and attachments (space admins)", res=resp(obj({"purged": num})))
op("/pages/{id}/restore", "post", "Pages", "Restore from the trash", res=NO)
op("/pages/{id}/children", "get", "Pages", "Child pages", query=[q("depth", "levels", num)], public=True)
op("/pages/{id}/versions", "get", "Pages", "Version history (minor: watchers weren't notified)")
op("/pages/{id}/versions", "post", "Pages", "Save a named version", req=body(obj({"message": txt, "minor": {"type": "boolean", "description": "don't notify watchers"}})))
op("/pages/{id}/minor-edit", "get", "Pages", "Whether my changes in the open editing session are minor", res=resp(obj({"minor": boolean})))
op("/pages/{id}/minor-edit", "put", "Pages", "Mark my changes in the open editing session as minor: when everybody who edited says so, its version doesn't notify watchers", req=body(obj({"minor": boolean}, ["minor"])), res=resp(obj({"minor": boolean})))
op("/pages/{id}/versions/{version}", "get", "Pages", "A past version")
op("/pages/{id}/versions/{version}/restore", "post", "Pages", "Restore a version (creates a new one)")
op("/pages/{id}/restrictions", "get", "Permissions", "Page view/edit restrictions (own and inherited)")
op("/pages/{id}/restrictions", "put", "Permissions", "Replace page restrictions", req=body(obj({"restrictions": arr(S("Restriction"))})), res=NO)
op("/pages/{id}/access/{who}", "get", "Permissions", "Explain why a user (or `anonymous`) can or can't view/edit the page")
op("/pages/{id}/labels", "get", "Pages", "Labels", res=resp(arr(txt)))
op("/pages/{id}/labels", "put", "Pages", "Replace labels", req=body(obj({"labels": arr(txt)})))
op("/pages/{id}/favorite", "put", "Pages", "Star", res=NO)
op("/pages/{id}/favorite", "delete", "Pages", "Unstar", res=NO)
op("/pages/{id}/like", "put", "Pages", "Like a page", res=resp(obj({"count": num, "liked": boolean, "names": arr(txt)})))
op("/pages/{id}/like", "delete", "Pages", "Remove your like", res=resp(obj({"count": num, "liked": boolean, "names": arr(txt)})))
VERIFICATION = obj({"status": {"enum": ["none", "verified", "expired"]}, "verifiedBy": obj({"id": uuid, "name": txt}), "verifiedAt": txt, "expiresAt": txt, "note": txt, "editedSince": boolean})
op("/pages/{id}/verification", "get", "Pages", "Verification state of a page", res=resp(VERIFICATION), public=True)
op("/pages/{id}/verification", "put", "Pages", "Verify a page (editors), optionally until a number of days from now", req=body(obj({"days": num, "note": txt})), res=resp(VERIFICATION))
op("/pages/{id}/verification", "delete", "Pages", "Remove the verification (editors)", res=resp(None, "204", "Removed"))
op("/pages/{id}/owner", "put", "Pages", "Hand the page over to someone who can edit it (page admins or the owner)", req=body(obj({"userId": uuid}, ["userId"])), res=resp(obj({"ownerId": uuid, "ownerName": txt})))
op("/pages/{id}/archive", "post", "Pages", "Archive a page and everything under it (read-only, hidden from tree and search)", res=resp(None, "204", "Archived"))
op("/pages/{id}/unarchive", "post", "Pages", "Restore an archived page", res=resp(None, "204", "Restored"))
op("/spaces/{key}/archived", "get", "Spaces", "Archived pages of a space", res=resp(arr(obj({"id": uuid, "title": txt, "archivedAt": txt, "archivedBy": txt, "descendants": num, "canRestore": boolean}))))
op("/spaces/{key}/star", "put", "Spaces", "Star a space (shown first in the directory and in the sidebar)", res=NO)
op("/spaces/{key}/star", "delete", "Spaces", "Unstar a space", res=NO)
op("/spaces/{key}/watch", "put", "Spaces", "Watch the space (new and updated pages)", res=NO)
op("/spaces/{key}/watch", "delete", "Spaces", "Stop watching the space", res=NO)
op("/pages/{id}/access-request", "post", "Pages", "Ask the page's admins for access (always 204; reveals nothing about the page)", req=body(obj({"message": txt})), res=resp(None, "204", "Sent"))
op("/pages/{id}/verification/request", "post", "Pages", "Ask someone who may edit the page to review and verify it", req=body(obj({"userId": uuid, "note": txt})), res=resp(None, "204", "Requested"))
op("/comments/{id}/like", "put", "Comments", "Like a comment", res=NO)
op("/comments/{id}/like", "delete", "Comments", "Remove your like", res=NO)
op("/pages/{id}/watch", "put", "Pages", "Watch (notifications); with includeChildren also the pages below it, including new ones (body optional)", req=body(obj({"includeChildren": boolean})), res=NO)
op("/pages/{id}/watch", "delete", "Pages", "Stop watching", res=NO)
op("/pages/{id}/export", "get", "Import & export", "Export one page (Markdown, HTML, Word or JSON)", query=[q("format", "md | html | docx | json"), q("tree", "docx: true = include every descendant page you can see")], public=True)
op("/pages/{id}/collab", "get", "Pages", "Live collaboration WebSocket (y-websocket protocol)", public=True)
op("/pages/{id}/tasks/{idx}/toggle", "post", "Tasks", "Check / uncheck a task item")
op("/pages/{id}/share-links", "get", "Sharing", "Public links of a page", res=resp(obj({"enabled": boolean, "links": arr(S("ShareLink"))})))
op("/pages/{id}/share-links", "post", "Sharing", "Create a public link", req=body(obj({"includeChildren": boolean, "expiresInDays": num, "password": {"type": "string", "description": "visitors must enter it (6 to 200 characters)"}})), res=resp(obj({"id": uuid, "token": txt, "url": txt}), "201", "Created"))
op("/share-links/{id}", "delete", "Sharing", "Turn off a public link", res=NO)
op("/public/{token}", "get", "Sharing", "A publicly shared page (rendered HTML + shared tree)", public=True)
op("/public/{token}/unlock", "post", "Sharing", "Enter the password of a protected public link: this browser may open it for 12 hours (a cookie); 10 wrong passwords per 15 minutes", req=body(obj({"password": txt}, ["password"])), res=NO, public=True)
op("/public/{token}/pages/{id}", "get", "Sharing", "A sub-page of a public share", public=True)
op("/public/{token}/attachments/{id}", "get", "Sharing", "A file of a publicly shared page", public=True)
# comments, attachments
op("/pages/{id}/comments", "get", "Comments", "Comments (incl. inline)", res=resp(arr(S("Comment"))), public=True)
op("/pages/{id}/comments", "post", "Comments", "Add a comment or reply", req=body(obj({"text": {"type": "string", "description": "Markdown"}, "parentId": uuid, "inline": anyobj})), res=resp(S("Comment"), "201", "Created"))
op("/comments/{id}", "patch", "Comments", "Edit, resolve or reopen", req=body(obj({"text": txt, "resolved": boolean})))
op("/comments/{id}", "delete", "Comments", "Delete (with replies)", res=NO)
op("/pages/{id}/attachments", "get", "Attachments", "Attachments of a page", res=resp(arr(S("Attachment"))), public=True)
op("/pages/{id}/attachments", "post", "Attachments", "Upload files (new versions when the name exists)", req=multipart({}), res=resp(arr(S("Attachment")), "201", "Created"))
op("/attachments/{id}", "delete", "Attachments", "Delete an attachment", res=NO)
op("/attachments/{id}/content", "get", "Attachments", "Download (images/PDF inline, everything else as download)", public=True)
op("/attachments/{id}/versions", "get", "Attachments", "Every uploaded version of a file, newest first", res=resp(arr(obj({"version": num, "size": num, "mediaType": txt, "createdByName": {"type": ["string", "null"]}, "createdAt": dt}))))
op("/attachments/{id}/versions/{version}/content", "get", "Attachments", "Download an earlier version", res={"200": {"description": "file", "content": {"application/octet-stream": {}}}})
op("/attachments/{id}/versions/{version}/restore", "post", "Attachments", "Make an earlier version current again (as a new version)", res=resp(obj({"version": num})))
# discovery
op("/search", "get", "Discovery", "Full-text search over visible pages", res=resp(arr(S("SearchHit"))), query=[q("q", "query"), q("space", "space key"), q("kind", "page | blog"), q("label", "label"), q("contributor", "user id (created or edited)"), q("days", "modified within N days"), q("limit", "max results")], public=True)
op("/recent", "get", "Discovery", "Recently viewed and recently updated pages")
op("/favorites", "get", "Discovery", "Starred pages", res=resp(arr(S("PageListItem"))))
op("/popular", "get", "Search", "Most viewed pages you can see (last N days, signed-in viewers)", query=[q("days", "1-365, default 30")])
op("/me/searches", "get", "Search", "Your saved searches", res=resp(arr(obj({"id": uuid, "name": txt, "params": txt}))))
op("/me/searches", "post", "Search", "Save a search (the search page's query string)", req=body(obj({"name": txt, "params": txt}, ["name", "params"])), res=resp(obj({"id": uuid, "name": txt, "params": txt}), "201", "Saved"))
op("/me/searches/{id}", "delete", "Search", "Delete a saved search", res=NO)
op("/space-templates", "get", "Spaces", "Space templates (pass `template` when creating a space)", res=resp(arr(obj({"id": txt, "name": txt, "icon": txt, "description": txt, "pages": arr(txt)}))))
op("/markdown", "post", "Pages", "Convert Markdown to document content (paste)", req=body(obj({"markdown": txt}, ["markdown"])), res=resp(obj({"content": anyobj})))
op("/unfurl", "get", "Pages", "Link preview of an external page (title, description, site name), fetched with SSRF protections and cached", query=[q("url", "http(s) URL")], res=resp(obj({"url": txt, "title": txt, "description": txt, "siteName": txt})))
op("/labels", "get", "Discovery", "Label names for autocomplete: the most used ones starting with q, over pages the caller can see", res=resp(arr(obj({"name": txt, "count": num}))), query=[q("q", "prefix")])
op("/labels/{name}", "get", "Discovery", "Pages with a label", res=resp(arr(S("PageListItem"))), public=True)
op("/resolve", "get", "Discovery", "Find a page by space + title or by imported Confluence id", query=[q("space", "space key"), q("title", "page title"), q("source", "confluence:<id>")], public=True)
op("/notifications", "get", "Notifications", "Notifications", res=resp(arr(S("Notification"))))
op("/notifications/read", "post", "Notifications", "Mark notifications as read", req=body(obj({"ids": arr(uuid)})), res=NO)
op("/tasks", "get", "Tasks", "Tasks assigned to me", res=resp(arr(S("Task"))), query=[q("status", "open | done | all")])
op("/reports/pages", "get", "Reports", "Pages by label / recently updated (contentbylabel, recently-updated)", query=[q("labels", "comma-separated"), q("spaces", "comma-separated keys"), q("operator", "all = every label"), q("sort", "title | updated | created"), q("max", "limit", num)], public=True)
op("/reports/properties", "get", "Reports", "Page properties report (detailssummary)", query=[q("labels", "comma-separated"), q("spaces", "comma-separated keys")], public=True)
op("/ai/status", "get", "Search", "Whether Ask the wiki is enabled", res=resp(obj({"enabled": boolean, "model": txt})), public=True)
op("/ai/ask", "post", "Search", "Ask the wiki: an answer from the pages you can see, with sources", req=body(obj({"question": txt, "spaceKey": txt}, ["question"])), res=resp(obj({"answer": txt, "answerHtml": txt, "sources": arr(obj({"n": num, "pageId": uuid, "title": txt, "spaceKey": txt, "spaceName": txt})), "model": txt})))
op("/reports/verification", "get", "Reports", "Verified pages by state", query=[q("status", "expired | verified | edited"), q("space", "space key")], public=True)
op("/reports/tasks", "get", "Reports", "Task report (tasks-report-macro)", query=[q("spaces", "keys"), q("labels", "labels"), q("status", "open | done | all")], public=True)
op("/reports/decisions", "get", "Reports", "Decision log (decisionreport): decision items of the matching pages", query=[q("spaces", "keys"), q("labels", "labels"), q("status", "decided = only decided items"), q("max", "rows")], public=True)
op("/templates", "get", "Templates", "Built-in and custom templates, each with the {{variables}} it asks for", query=[q("space", "space key")], res=resp(arr(obj({"id": txt, "name": txt, "description": {"type": ["string", "null"]}, "icon": {"type": ["string", "null"]}, "builtin": boolean, "spaceKey": {"type": ["string", "null"]}, "variables": arr(txt)}))))
op("/templates", "post", "Templates", "Save a template", req=body(obj({"name": txt, "description": txt, "spaceKey": txt, "content": S("PmNode"), "pageId": uuid})))
op("/templates/{id}", "delete", "Templates", "Delete a template", res=NO)
op("/templates/{id}/preview", "get", "Templates", "What a page from the template looks like (title and rendered HTML) and the {{variables}} it asks for", res=resp(obj({"title": {"type": ["string", "null"]}, "html": txt, "variables": arr(txt)})))
# import / export / jobs
op("/imports/confluence", "post", "Import & export", "Import a Confluence space export (XML or Cloud CSV zip) as a job", req=multipart({"options": {"type": "string", "description": "JSON ImportOptions"}}), res=resp(obj({"jobId": uuid}), "202", "Queued"))
op("/imports/confluence/analyze", "post", "Import & export", "Dry run: what an import would create", req=multipart({}))
op("/imports/markdown", "post", "Import & export", "Import Word, Markdown, HTML, Notion or Obsidian (.docx, .md, .html or a .zip of them, incl. Confluence HTML exports)", req=multipart({"spaceKey": txt, "parentId": uuid}), res=resp(obj({"pages": num, "attachments": num}), "201", "Created"))
op("/imports/backup", "post", "Import & export", "Restore a space backup as a new space (job)", req=multipart({"spaceKey": txt, "spaceName": txt}), res=resp(obj({"jobId": uuid, "spaceKey": txt}), "202", "Queued"))
op("/jobs", "get", "Import & export", "Recent jobs (admins)", res=resp(arr(S("Job"))))
op("/jobs/{id}", "get", "Import & export", "A job", res=resp(S("Job")))
op("/jobs/{id}/download", "get", "Import & export", "Download an export archive (requester, 24 h)")
# admin
op("/admin/audit", "get", "Admin", "Audit log", query=[q("before", "id cursor", num), q("action", "action prefix")])
op("/admin/stats", "get", "Admin", "Workspace statistics")
op("/admin/reindex", "post", "Admin", "Rebuild the search index")
op("/admin/spaces", "get", "Admin", "All spaces incl. archived, with size and activity")
op("/admin/spaces/{key}", "delete", "Admin", "Permanently delete an archived space", req=body(obj({"confirm": {"type": "string", "description": "the space key"}})), res=NO)
op("/admin/spaces/{key}/unarchive", "post", "Admin", "Restore an archived space", res=NO)
SWITCHES = obj({"publicLinks": boolean, "exports": boolean, "ai": boolean, "personalSpaces": boolean, "trashRetentionDays": {"type": "integer", "minimum": 0, "maximum": 3650, "description": "days pages stay in the trash before they are deleted for good; 0 = forever"}, "viewerList": {"type": "boolean", "description": "editors can see who viewed a page (default off)"}, "requireTwoFactor": {"enum": ["off", "admins", "everyone"], "description": "who must set up two-factor authentication before using the wiki"}})
op("/admin/settings", "get", "Admin", "Workspace switches (public links, exports, Ask the wiki, personal spaces) and the trash retention period", res=resp(SWITCHES))
op("/admin/settings", "put", "Admin", "Turn workspace switches on or off (only the ones named change)", req=body(SWITCHES), res=resp(SWITCHES))
op("/features", "get", "Admin", "The switches as they apply to the caller (admins can always export)", res=resp(SWITCHES))
op("/activity", "get", "Search", "Activity feed: pages created, edited and commented on, newest first (visible pages only)", query=[q("scope", "all | watching"), q("limit", "events (max 100)")], res=resp(arr(obj({"kind": {"enum": ["created", "edited", "commented"]}, "pageId": uuid, "pageTitle": txt, "spaceKey": txt, "spaceName": txt, "actorName": {"type": ["string", "null"]}, "at": dt, "detail": {"type": ["string", "null"]}, "count": num}))))
op("/admin/workspace", "put", "Admin", "Rename the workspace", req=body(obj({"name": txt}, ["name"])), res=resp(obj({"name": txt})))
op("/admin/workspace/logo", "post", "Admin", "Upload the workspace logo (PNG, JPEG or WebP, up to 1 MB)", req=body(obj({"file": {"type": "string", "format": "binary"}}), "multipart/form-data"), res=resp(obj({"logoVersion": txt})))
op("/admin/workspace/logo", "delete", "Admin", "Remove the workspace logo")
op("/workspace/logo", "get", "Admin", "The workspace logo image", public=True)
op("/admin/mail", "get", "Admin", "E-mail configuration and outbox status")
op("/admin/mail/test", "post", "Admin", "Send a test e-mail", req=body(obj({"to": txt})))
op("/admin/webhooks", "get", "Webhooks", "Webhooks and available events", res=resp(obj({"hooks": arr(S("Webhook")), "events": arr(txt)})))
op("/admin/webhooks", "post", "Webhooks", "Add a webhook (returns the signing secret once)", req=body(obj({"name": txt, "url": txt, "events": arr(txt), "spaceKey": txt, "active": boolean, "format": {"enum": ["json", "slack", "teams", "googlechat"]}}, ["url"])), res=resp(obj({"id": uuid, "secret": txt}), "201", "Created"))
op("/admin/webhooks/{id}", "patch", "Webhooks", "Change a webhook", req=body(obj({"name": txt, "url": txt, "events": arr(txt), "spaceKey": txt, "active": boolean, "format": {"enum": ["json", "slack", "teams", "googlechat"]}})), res=NO)
op("/admin/webhooks/{id}", "delete", "Webhooks", "Delete a webhook", res=NO)
op("/admin/webhooks/{id}/test", "post", "Webhooks", "Queue a ping event", res={"202": {"description": "Queued"}})
op("/admin/webhooks/{id}/deliveries", "get", "Webhooks", "Latest deliveries")
op("/health", "get", "Operations", "Liveness", public=True)
op("/ready", "get", "Operations", "Readiness (503 while shutting down or without database)", public=True)
op("/metrics", "get", "Operations", "Prometheus metrics (bearer WIKICRAFT_METRICS_TOKEN when set)", public=True)
op("/openapi.yaml", "get", "Operations", "This document", public=True)

paths = {}
for path, method, tag, summary, req, res, public, desc, query in R:
    params = [{"name": p.strip("{}*"), "in": "path", "required": True, "schema": txt} for p in path.split("/") if p.startswith("{")]
    o = {"tags": [tag], "summary": summary, "operationId": (method + "_" + path.strip("/").replace("/", "_").replace("{", "").replace("}", "").replace("-", "_").replace(".", "_")) or method}
    if params or query:
        o["parameters"] = params + (query or [])
    if req:
        o["requestBody"] = req
    o["responses"] = {**(res or resp()), "4XX": {"description": "Error", "content": {"application/json": {"schema": S("Error")}}}}
    if public:
        o["security"] = [{}, {"session": []}, {"token": []}]
    paths.setdefault(path, {})[method] = o

doc = {
    "openapi": "3.1.0",
    "info": {
        "title": "WikiCraft API",
        "version": "0.1.0",
        "description": "All endpoints live under /api. Authenticate with a personal API token (`Authorization: Bearer wct_…`, create one under Settings) or the browser session cookie; cookie-authenticated requests that change data must send `x-wikicraft: 1`. Content is ProseMirror/Tiptap JSON (or Markdown where noted). Results are always filtered by the caller's permissions. Webhook payloads use the WebhookEvent schema.",
    },
    "servers": [{"url": "/api/v1", "description": "versioned (integrations)"}, {"url": "/api", "description": "unversioned alias used by the web app"}],
    "security": [{"token": []}, {"session": []}],
    "components": {
        "securitySchemes": {
            "token": {"type": "http", "scheme": "bearer", "description": "Personal API token (wct_…)"},
            "session": {"type": "apiKey", "in": "cookie", "name": "wc_session"},
        },
        "schemas": schemas,
    },
    "paths": paths,
}
out = pathlib.Path(__file__).resolve().parent.parent / "crates/server/openapi.yaml"
out.write_text("# Generated by scripts/gen-openapi.py — edit the script, not this file.\n" + yaml.safe_dump(doc, sort_keys=False, allow_unicode=True, width=120))
print(f"{len(R)} operations, {len(paths)} paths -> {out}")
