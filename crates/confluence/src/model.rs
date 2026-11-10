//! Normalized, format-independent model of a Confluence space export.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// Classic `entities.xml` export (Server/DC, older Cloud).
    Xml,
    /// Cloud CSV export (`exportFormat=csv`).
    Csv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Page,
    BlogPost,
    Comment,
    Attachment,
    Folder,
    SpaceDescription,
    Whiteboard,
    Database,
    Embed,
    Custom,
    Other,
}

impl ContentKind {
    pub fn from_xml_class(class: &str) -> Option<Self> {
        Some(match class {
            "Page" => Self::Page,
            "BlogPost" => Self::BlogPost,
            "Comment" => Self::Comment,
            "Attachment" => Self::Attachment,
            "SpaceDescription" | "PersonalInformation" => Self::SpaceDescription,
            "CustomContentEntityObject" => Self::Custom,
            "Folder" => Self::Folder,
            "Whiteboard" => Self::Whiteboard,
            "Database" => Self::Database,
            "Embed" | "EmbedContentEntityObject" => Self::Embed,
            "Draft" => Self::Page,
            _ => return None,
        })
    }

    pub fn from_csv_type(t: &str) -> Self {
        match t {
            "PAGE" => Self::Page,
            "BLOGPOST" => Self::BlogPost,
            "COMMENT" => Self::Comment,
            "ATTACHMENT" => Self::Attachment,
            "FOLDER" => Self::Folder,
            "SPACEDESCRIPTION" | "USERINFO" => Self::SpaceDescription,
            "WHITEBOARD" => Self::Whiteboard,
            "DATABASE" => Self::Database,
            "EMBED" => Self::Embed,
            "CUSTOM" => Self::Custom,
            _ => Self::Other,
        }
    }

    /// Whether this content lives in the page tree.
    pub fn is_tree_node(self) -> bool {
        matches!(self, Self::Page | Self::Folder | Self::Whiteboard | Self::Database | Self::Embed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentStatus {
    Current,
    Draft,
    Deleted,
    Archived,
    Other,
}

impl ContentStatus {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "current" => Self::Current,
            "draft" => Self::Draft,
            "deleted" => Self::Deleted,
            "archived" => Self::Archived,
            _ => Self::Other,
        }
    }
}

/// Body representation types (`bodyType` / `bodytypeid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// 0 — legacy wiki markup.
    WikiMarkup,
    /// 1 — raw/JSON (content properties and similar).
    Raw,
    /// 2 — XHTML storage format.
    Storage,
}

impl BodyKind {
    pub fn from_code(code: &str) -> Self {
        match code.trim() {
            "0" => Self::WikiMarkup,
            "2" => Self::Storage,
            _ => Self::Raw,
        }
    }

    /// For bodies without a type (seen in old exports): markup that starts with a tag is
    /// storage format, anything else is wiki markup (Confluence before 4.0).
    pub fn guess(code: Option<&str>, body: &str) -> Self {
        match code.map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => Self::from_code(c),
            None if body.trim_start().starts_with('<') => Self::Storage,
            None => Self::WikiMarkup,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Space {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description_id: Option<String>,
    pub home_page_id: Option<String>,
    pub creator: Option<String>,
    pub created: Option<String>,
    pub space_type: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Content {
    pub id: String,
    pub kind: ContentKind,
    pub title: String,
    pub status: ContentStatus,
    pub version: i64,
    /// For historical versions: the id of the current (latest) content object.
    pub original_version_id: Option<String>,
    pub parent_id: Option<String>,
    pub position: Option<i64>,
    pub space_id: Option<String>,
    /// Container (page/blog) of a comment or attachment.
    pub container_id: Option<String>,
    pub parent_comment_id: Option<String>,
    pub creator: Option<String>,
    pub created: Option<String>,
    pub last_modifier: Option<String>,
    pub modified: Option<String>,
    pub version_comment: Option<String>,
    pub plugin_key: Option<String>,
    /// XML exports: ids of ContentProperty objects attached to this content.
    #[serde(skip)]
    pub property_ids: Vec<String>,
}

impl Content {
    pub fn is_historical(&self) -> bool {
        self.original_version_id.is_some()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Body {
    pub content_id: String,
    pub kind: BodyKind,
    pub body: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct User {
    /// Confluence user key (`ri:userkey`) or Atlassian account id.
    pub key: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Labelling {
    pub content_id: String,
    pub name: String,
    pub namespace: String,
    /// user key of a personal (`my:`) label's owner
    pub owner: Option<String>,
}

/// Someone watching a page or blog post (`content_id`) or a whole space (`space_id`).
#[derive(Debug, Clone, Serialize)]
pub struct Watch {
    pub user_key: String,
    pub content_id: Option<String>,
    pub space_id: Option<String>,
}

/// A page (or, through its description, a space) someone marked as favourite / saved for later.
#[derive(Debug, Clone, Serialize)]
pub struct Favourite {
    pub user_key: String,
    pub content_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpacePermission {
    pub space_id: String,
    pub kind: String,
    pub user_key: Option<String>,
    pub group: Option<String>,
    /// No user and no group: anonymous (public) access.
    pub anonymous: bool,
    /// `allUsersSubject`: all logged-in users.
    pub all_users: bool,
}

impl SpacePermission {
    /// A grant from its subject columns: a user, a group, or `allUsersSubject`, whose value is
    /// `all-licensed-users` / `anonymous-users` / `all-product-admins` in current exports and a
    /// boolean in old ones. A grant to product admins is left out (both flags off): workspace
    /// admins have access anyway.
    pub fn new(space_id: String, kind: String, user_key: Option<String>, group: Option<String>, all_users_subject: Option<&str>) -> Self {
        let subject = all_users_subject.map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty());
        let (all_users, anonymous) = match subject.as_deref() {
            Some("all-licensed-users" | "all-users" | "true" | "t" | "1") => (true, false),
            Some("anonymous-users" | "anonymous") => (false, true),
            Some("false" | "f" | "0") | None => (false, user_key.is_none() && group.is_none()),
            Some(_) => (false, false),
        };
        Self { space_id, kind, user_key, group, anonymous, all_users }
    }
}

/// Whether an `active` column / property switches a row off (absent means active).
pub fn inactive(active: Option<&str>) -> bool {
    active.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "f" | "0"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestrictionKind {
    View,
    Edit,
}

#[derive(Debug, Clone, Serialize)]
pub struct Restriction {
    pub content_id: String,
    pub kind: RestrictionKind,
    pub user_key: Option<String>,
    pub group: Option<String>,
}

/// Everything read from one export.
#[derive(Debug, Default)]
pub struct Export {
    pub format: Option<ExportFormat>,
    pub descriptor: BTreeMap<String, String>,
    pub spaces: Vec<Space>,
    pub contents: BTreeMap<String, Content>,
    /// Best body per content id (storage preferred).
    pub bodies: HashMap<String, Body>,
    pub users: HashMap<String, User>,
    pub labellings: Vec<Labelling>,
    pub space_permissions: Vec<SpacePermission>,
    pub restrictions: Vec<Restriction>,
    pub watches: Vec<Watch>,
    pub favourites: Vec<Favourite>,
    /// content id → (property name → string value)
    pub properties: HashMap<String, BTreeMap<String, String>>,
    /// Paths of attachment binaries inside the export, keyed by attachment content id → version → path.
    pub attachment_files: HashMap<String, BTreeMap<i64, String>>,
    /// Non-fatal problems encountered while reading.
    pub read_warnings: Vec<String>,
}

impl Export {
    pub fn body(&self, content_id: &str) -> Option<&Body> {
        self.bodies.get(content_id)
    }

    pub fn property(&self, content_id: &str, name: &str) -> Option<&str> {
        self.properties.get(content_id).and_then(|p| p.get(name)).map(String::as_str)
    }

    pub fn space_by_id(&self, id: &str) -> Option<&Space> {
        self.spaces.iter().find(|s| s.id == id)
    }

    /// Records a body, preferring storage format over other representations.
    pub(crate) fn add_body(&mut self, body: Body) {
        match self.bodies.get(&body.content_id) {
            Some(existing) if existing.kind == BodyKind::Storage && body.kind != BodyKind::Storage => {}
            _ => {
                self.bodies.insert(body.content_id.clone(), body);
            }
        }
    }

    /// Path of the attachment binary for a given attachment id/version (falls back to latest).
    pub fn attachment_path(&self, attachment_id: &str, version: i64) -> Option<&str> {
        let versions = self.attachment_files.get(attachment_id)?;
        versions.get(&version).or_else(|| versions.values().next_back()).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_permission_subjects() {
        let p = |user: Option<&str>, group: Option<&str>, all: Option<&str>| {
            let p = SpacePermission::new("1".into(), "VIEWSPACE".into(), user.map(String::from), group.map(String::from), all);
            (p.all_users, p.anonymous)
        };
        assert_eq!(p(None, None, Some("all-licensed-users")), (true, false));
        assert_eq!(p(None, None, Some("anonymous-users")), (false, true));
        assert_eq!(p(None, None, Some("all-product-admins")), (false, false));
        assert_eq!(p(None, None, Some("true")), (true, false), "old exports");
        assert_eq!(p(None, None, None), (false, true), "no subject at all: anonymous");
        assert_eq!(p(Some("u1"), None, None), (false, false));
        assert_eq!(p(None, Some("devs"), Some("")), (false, false));
        assert!(inactive(Some("false")) && inactive(Some("f")) && !inactive(Some("t")) && !inactive(None));
    }
}
