//! Streaming reader for classic Confluence XML exports (`entities.xml`).
//!
//! Structure (Hibernate dump):
//! ```xml
//! <hibernate-generic>
//!   <object class="Page" package="...">
//!     <id name="id">123</id>
//!     <property name="title"><![CDATA[Title]]></property>
//!     <property name="space" class="Space"><id name="id">9</id></property>
//!     <collection name="contentProperties"><element class="ContentProperty"><id name="id">5</id></element></collection>
//!   </object>
//! </hibernate-generic>
//! ```

use std::collections::HashMap;
use std::io::{BufReader, Read};

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::model::*;

#[derive(Debug, Default)]
struct RawObject {
    class: String,
    id: String,
    props: HashMap<String, String>,
    collections: HashMap<String, Vec<String>>,
}

impl RawObject {
    /// Trimmed property value (reference properties contain the referenced `<id>` plus whitespace).
    fn get(&self, k: &str) -> Option<String> {
        self.props.get(k).map(|v| v.trim()).filter(|v| !v.is_empty()).map(String::from)
    }
}

/// Collections we keep element ids for.
const KEEP_COLLECTIONS: &[&str] = &["contentProperties"];

#[derive(Default)]
struct Pending {
    // ContentProperty id → (name, value)
    content_properties: HashMap<String, (String, String)>,
    labels: HashMap<String, (String, String)>,             // label id → (name, namespace)
    labellings: Vec<(String, String, Option<String>)>,     // (label id, content id, owner)
    perm_sets: HashMap<String, (String, RestrictionKind)>, // set id → (content id, kind)
    perms: Vec<(String, Option<String>, Option<String>)>,  // (set id, user key, group)
    // Crowd users: (username, display name, e-mail); they can come before the Confluence users
    crowd_users: Vec<(String, Option<String>, Option<String>)>,
}

pub fn read<R: Read>(input: R, export: &mut Export) -> Result<(), crate::Error> {
    let mut reader = Reader::from_reader(BufReader::with_capacity(1 << 20, input));
    reader.config_mut().trim_text(false);
    let mut buf = Vec::with_capacity(1 << 16);
    let mut pending = Pending::default();

    // parser state
    let mut depth = 0usize;
    let mut obj: Option<RawObject> = None;
    let mut prop_name: Option<String> = None;
    let mut prop_text = String::new();
    let mut in_obj_id = false;
    let mut coll_name: Option<String> = None;
    let mut in_elem_id = false;
    let mut elem_text = String::new();

    loop {
        let ev = reader.read_event_into(&mut buf).map_err(|e| crate::Error::Xml(format!("entities.xml at byte {}: {e}", reader.buffer_position())))?;
        match ev {
            Event::Start(e) => {
                depth += 1;
                let name = e.name();
                match (depth, name.as_ref()) {
                    (2, "object") => {
                        let class = attr(&e, "class").unwrap_or_default();
                        obj = Some(RawObject { class, ..Default::default() });
                    }
                    (3, "id") if obj.is_some() => {
                        in_obj_id = true;
                        prop_text.clear();
                    }
                    (3, "property") if obj.is_some() => {
                        prop_name = attr(&e, "name");
                        prop_text.clear();
                    }
                    (3, "collection") if obj.is_some() => {
                        let n = attr(&e, "name").unwrap_or_default();
                        coll_name = KEEP_COLLECTIONS.contains(&n.as_str()).then_some(n);
                    }
                    (5, "id") if coll_name.is_some() => {
                        in_elem_id = true;
                        elem_text.clear();
                    }
                    _ => {}
                }
            }
            Event::Empty(e) => {
                // <property name="x"/> → empty value
                if depth == 2
                    && e.name().as_ref() == "property"
                    && let (Some(o), Some(n)) = (obj.as_mut(), attr(&e, "name"))
                {
                    o.props.insert(n, String::new());
                }
            }
            Event::End(e) => {
                let name = e.name();
                match (depth, name.as_ref()) {
                    (2, "object") => {
                        if let Some(o) = obj.take() {
                            dispatch(o, export, &mut pending);
                        }
                    }
                    (3, "id") if in_obj_id => {
                        if let Some(o) = obj.as_mut() {
                            o.id = prop_text.trim().to_string();
                        }
                        in_obj_id = false;
                    }
                    (3, "property") => {
                        if let (Some(o), Some(n)) = (obj.as_mut(), prop_name.take()) {
                            o.props.insert(n, std::mem::take(&mut prop_text));
                        }
                    }
                    (3, "collection") => coll_name = None,
                    (5, "id") if in_elem_id => {
                        if let (Some(o), Some(c)) = (obj.as_mut(), coll_name.as_ref()) {
                            o.collections.entry(c.clone()).or_default().push(elem_text.trim().to_string());
                        }
                        in_elem_id = false;
                    }
                    _ => {}
                }
                depth = depth.saturating_sub(1);
            }
            Event::Text(t) => append(&mut prop_text, &mut elem_text, in_elem_id, prop_name.is_some() || in_obj_id, &t.into_inner()),
            Event::CData(t) => append(&mut prop_text, &mut elem_text, in_elem_id, prop_name.is_some() || in_obj_id, &t.into_inner()),
            Event::GeneralRef(r) => {
                let s = if let Ok(Some(ch)) = r.resolve_char_ref() {
                    ch.to_string()
                } else {
                    match r.into_inner().as_ref() {
                        "amp" => "&".to_string(),
                        "lt" => "<".to_string(),
                        "gt" => ">".to_string(),
                        "quot" => "\"".to_string(),
                        "apos" => "'".to_string(),
                        other => format!("&{other};"),
                    }
                };
                append(&mut prop_text, &mut elem_text, in_elem_id, prop_name.is_some() || in_obj_id, &s);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    resolve_pending(pending, export);
    Ok(())
}

fn append(prop: &mut String, elem: &mut String, in_elem: bool, in_prop: bool, s: &str) {
    if in_elem {
        elem.push_str(s);
    } else if in_prop {
        prop.push_str(s);
    }
}

fn attr(e: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .map(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.to_string()).unwrap_or_else(|_| a.value.to_string()))
}

fn parse_i64(s: Option<String>) -> Option<i64> {
    s.and_then(|v| v.trim().parse().ok())
}

fn dispatch(o: RawObject, export: &mut Export, pending: &mut Pending) {
    if let Some(kind) = ContentKind::from_xml_class(&o.class) {
        let status = o.get("contentStatus").map(|s| ContentStatus::parse(&s)).unwrap_or(ContentStatus::Current);
        let content = Content {
            id: o.id.clone(),
            kind,
            title: o.get("title").unwrap_or_default(),
            status,
            version: parse_i64(o.get("version")).unwrap_or(1),
            original_version_id: o.get("originalVersion"),
            parent_id: o.get("parent"),
            position: parse_i64(o.get("position")),
            space_id: o.get("space"),
            container_id: o.get("containerContent").or_else(|| o.get("content")).or_else(|| o.get("page")),
            parent_comment_id: if o.class == "Comment" { o.get("parent") } else { None },
            creator: o.get("creator").or_else(|| o.get("creatorName")),
            created: o.get("creationDate"),
            last_modifier: o.get("lastModifier").or_else(|| o.get("lastModifierName")),
            modified: o.get("lastModificationDate"),
            version_comment: o.get("versionComment"),
            plugin_key: o.get("pluginModuleKey"),
            property_ids: o.collections.get("contentProperties").cloned().unwrap_or_default(),
        };
        // a Comment's `parent` is the parent comment, not a page
        let mut content = content;
        if kind == ContentKind::Comment {
            content.parent_id = None;
        }
        export.contents.insert(content.id.clone(), content);
        return;
    }
    match o.class.as_str() {
        "Space" => export.spaces.push(Space {
            id: o.id.clone(),
            key: o.get("key").unwrap_or_default(),
            name: o.get("name").unwrap_or_default(),
            description_id: o.get("description"),
            home_page_id: o.get("homePage"),
            creator: o.get("creator"),
            created: o.get("creationDate"),
            space_type: o.get("spaceType"),
            status: o.get("spaceStatus"),
        }),
        "BodyContent" => {
            if let Some(cid) = o.get("content") {
                let body = o.props.get("body").cloned().unwrap_or_default();
                export.add_body(Body { content_id: cid, kind: BodyKind::guess(o.get("bodyType").as_deref(), &body), body });
            }
        }
        "ConfluenceUserImpl" => {
            let key = o.id.clone();
            let u = export.users.entry(key.clone()).or_insert_with(|| User { key, ..Default::default() });
            u.username = o.get("name").or(u.username.take());
            u.email = o.get("email").or(u.email.take());
            u.account_id = o.get("atlassianAccountId").or(u.account_id.take());
        }
        "InternalUser" | "User" => {
            // Crowd user (full-site exports): username → display name / email
            if let Some(name) = o.get("name") {
                pending.crowd_users.push((name, o.get("displayName"), o.get("emailAddress")));
            }
        }
        "ContentProperty" => {
            let value = o.get("stringValue").or_else(|| o.get("longValue")).or_else(|| o.get("dateValue")).unwrap_or_default();
            pending.content_properties.insert(o.id.clone(), (o.get("name").unwrap_or_default(), value));
        }
        "Label" => {
            pending.labels.insert(o.id.clone(), (o.get("name").unwrap_or_default(), o.get("namespace").unwrap_or_else(|| "global".into())));
        }
        "Labelling" => {
            if let (Some(l), Some(c)) = (o.get("label"), o.get("content").or_else(|| o.get("labelableId"))) {
                pending.labellings.push((l, c, o.get("owningUser").or_else(|| o.get("owner"))));
            }
        }
        // watches: of a page / blog post (`content`) or of a whole space (`space`, no type or
        // type "page"); digests, network follows, label watches and blog-only space watches are
        // something else
        "Notification" => {
            let flag = |k: &str| o.get(k).is_some_and(|v| v == "true" || v == "1");
            let ty = o.get("type");
            if let Some(user) = o.get("receiver").or_else(|| o.get("userName")).or_else(|| o.get("username"))
                && !flag("digest")
                && !flag("network")
                && o.get("label").is_none()
            {
                match (o.get("content"), o.get("space")) {
                    (Some(c), _) => export.watches.push(Watch { user_key: user, content_id: Some(c), space_id: None }),
                    (None, Some(s)) if ty.as_deref().is_none_or(|t| t == "page") => {
                        export.watches.push(Watch { user_key: user, content_id: None, space_id: Some(s) })
                    }
                    _ => {}
                }
            }
        }
        "User2ContentRelationEntity" => {
            if o.get("relationName").is_some_and(|r| r.eq_ignore_ascii_case("favourite") || r.eq_ignore_ascii_case("favorite"))
                && let (Some(user), Some(c)) = (o.get("sourceContent"), o.get("targetContent"))
            {
                export.favourites.push(Favourite { user_key: user, content_id: c });
            }
        }
        "SpacePermission" => {
            if !inactive(o.get("active").as_deref()) {
                export.space_permissions.push(SpacePermission::new(
                    o.get("space").unwrap_or_default(),
                    o.get("type").unwrap_or_default(),
                    o.get("userSubject"),
                    o.get("group"),
                    o.get("allUsersSubject").as_deref(),
                ));
            }
        }
        "ContentPermissionSet" => {
            if let Some(c) = o.get("owningContent") {
                let kind = if o.get("type").unwrap_or_default().to_ascii_lowercase().contains("edit") { RestrictionKind::Edit } else { RestrictionKind::View };
                pending.perm_sets.insert(o.id.clone(), (c, kind));
            }
        }
        "ContentPermission" => {
            if let Some(set) = o.get("owningSet") {
                pending.perms.push((set, o.get("userSubject"), o.get("groupName")));
            }
        }
        _ => {}
    }
}

fn resolve_pending(p: Pending, export: &mut Export) {
    let by_username: HashMap<String, String> = export.users.values().filter_map(|u| Some((u.username.as_deref()?.to_lowercase(), u.key.clone()))).collect();
    for (name, display_name, email) in p.crowd_users {
        if let Some(u) = by_username.get(&name.to_lowercase()).and_then(|k| export.users.get_mut(k)) {
            u.display_name = display_name.or(u.display_name.take());
            u.email = email.or(u.email.take());
        }
    }
    // content properties are linked from the owner's `contentProperties` collection
    for c in export.contents.values() {
        for pid in &c.property_ids {
            if let Some((name, value)) = p.content_properties.get(pid) {
                export.properties.entry(c.id.clone()).or_default().insert(name.clone(), value.clone());
            }
        }
    }
    for (label_id, content_id, owner) in p.labellings {
        if let Some((name, ns)) = p.labels.get(&label_id) {
            export.labellings.push(Labelling { content_id, name: name.clone(), namespace: ns.clone(), owner });
        }
    }
    for (set, user, group) in p.perms {
        if let Some((content_id, kind)) = p.perm_sets.get(&set) {
            export.restrictions.push(Restriction { content_id: content_id.clone(), kind: *kind, user_key: user, group });
        }
    }
}

/// Indexes attachment binaries laid out as `attachments/<containerId>/<attachmentId>/<version>`
/// (also accepts `attachments/<attachmentId>/<version>`).
/// Newer Cloud backups store one file per attachment version as `attachments/<FILESTORE_ID>`.
pub fn index_attachments(names: &[String], export: &mut Export) {
    let mut by_filestore_id: HashMap<&str, &String> = HashMap::new();
    for n in names {
        let Some(rest) = n.strip_prefix("attachments/") else { continue };
        let parts: Vec<&str> = rest.split('/').collect();
        let (att, ver) = match parts.as_slice() {
            [_container, att, ver] => (*att, *ver),
            [att, ver] => (*att, *ver),
            [file] if !file.is_empty() => {
                by_filestore_id.insert(file, n);
                continue;
            }
            _ => continue,
        };
        let Ok(ver) = ver.parse::<i64>() else { continue };
        export.attachment_files.entry(att.to_string()).or_default().insert(ver, n.clone());
    }
    if by_filestore_id.is_empty() {
        return;
    }
    let found: Vec<(String, i64, String)> = export
        .contents
        .values()
        .filter(|c| c.kind == ContentKind::Attachment)
        .filter_map(|c| {
            let id = export.properties.get(&c.id)?.get("FILESTORE_ID")?;
            by_filestore_id.get(id.as_str()).map(|path| (c.id.clone(), c.version, (*path).clone()))
        })
        .collect();
    for (id, version, path) in found {
        export.attachment_files.entry(id).or_default().entry(version).or_insert(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crowd_users_before_confluence_users_still_name_them() {
        // Cloud backups list some Crowd users ahead of the Confluence users they belong to
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><hibernate-generic>
<object class="InternalUser" package="com.atlassian.crowd.model.user"><id name="id">1</id>
<property name="name"><![CDATA[jane@example.test]]></property><property name="displayName"><![CDATA[Jane Doe]]></property>
<property name="emailAddress"><![CDATA[jane@example.test]]></property></object>
<object class="ConfluenceUserImpl" package="com.atlassian.confluence.user"><id name="key"><![CDATA[k1]]></id>
<property name="name"><![CDATA[Jane@Example.test]]></property></object>
<object class="ConfluenceUserImpl" package="com.atlassian.confluence.user"><id name="key"><![CDATA[k2]]></id>
<property name="name"><![CDATA[sam]]></property></object>
<object class="InternalUser" package="com.atlassian.crowd.model.user"><id name="id">2</id>
<property name="name"><![CDATA[sam]]></property><property name="displayName"><![CDATA[Sam Smith]]></property></object>
</hibernate-generic>"#;
        let mut export = Export::default();
        read(xml.as_bytes(), &mut export).unwrap();
        assert_eq!(export.users["k1"].display_name.as_deref(), Some("Jane Doe"));
        assert_eq!(export.users["k1"].email.as_deref(), Some("jane@example.test"));
        assert_eq!(export.users["k2"].display_name.as_deref(), Some("Sam Smith"));
        assert_eq!(export.users["k2"].email, None);
    }
}
