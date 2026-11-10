//! Reader for Confluence Cloud CSV space exports (`exportFormat=csv`).
//!
//! Verified against a real export (Oct 2026): `spaces.csv.gz`, `content.csv.gz`,
//! `bodycontent.csv.gz`, `export-users.csv`; and against the two complete exports of the XWiki
//! test corpus (`tests/xwiki_corpus.rs`): `label`, `content_label`, `notifications`,
//! `usercontent_relation`, `spacepermissions`, `content_perm_set` / `content_perm`,
//! `contentproperties`, `user_mapping` use the Confluence database column names read here.

use std::collections::HashMap;
use std::io::Read;

use crate::model::*;
use crate::source::Source;

type Row = HashMap<String, String>;

/// Finds `<table>.csv` or `<table>.csv.gz` (case-insensitive), at the root or one folder deep.
fn find(source: &Source, names: &[String], table: &str) -> Option<String> {
    let want = [format!("{table}.csv"), format!("{table}.csv.gz")];
    let _ = source;
    names
        .iter()
        .filter(|n| {
            let base = n.rsplit('/').next().unwrap_or(n).to_ascii_lowercase();
            want.contains(&base)
        })
        .min_by_key(|n| n.len())
        .cloned()
}

fn for_each_row(source: &Source, name: &str, mut f: impl FnMut(Row)) -> Result<usize, crate::Error> {
    let input: Box<dyn Read + Send> = source.open_entry(name)?;
    let mut rdr = csv::ReaderBuilder::new().flexible(true).has_headers(true).from_reader(input);
    let headers: Vec<String> = rdr.headers().map_err(|e| crate::Error::Csv(format!("{name}: {e}")))?.iter().map(|h| h.trim().to_ascii_lowercase()).collect();
    let mut n = 0;
    for rec in rdr.records() {
        let rec = rec.map_err(|e| crate::Error::Csv(format!("{name}: {e}")))?;
        let row: Row = headers.iter().cloned().zip(rec.iter().map(String::from)).collect();
        f(row);
        n += 1;
    }
    Ok(n)
}

fn get(r: &Row, k: &str) -> Option<String> {
    r.get(k).map(|s| s.trim()).filter(|s| !s.is_empty() && *s != "\\N" && *s != "NULL").map(String::from)
}

fn get_i64(r: &Row, k: &str) -> Option<i64> {
    get(r, k).and_then(|v| v.parse().ok())
}

pub fn read(source: &Source, export: &mut Export) -> Result<(), crate::Error> {
    let names = source.names();

    if let Some(n) = find(source, &names, "spaces") {
        for_each_row(source, &n, |r| {
            export.spaces.push(Space {
                id: get(&r, "spaceid").unwrap_or_default(),
                key: get(&r, "spacekey").unwrap_or_default(),
                name: get(&r, "spacename").unwrap_or_default(),
                description_id: get(&r, "spacedescid"),
                home_page_id: get(&r, "homepage"),
                creator: get(&r, "creator"),
                created: get(&r, "creationdate"),
                space_type: get(&r, "spacetype"),
                status: get(&r, "spacestatus"),
            });
        })?;
    } else {
        export.read_warnings.push("spaces.csv not found".into());
    }

    let content_file = find(source, &names, "content").ok_or_else(|| crate::Error::Format("content.csv(.gz) not found in CSV export".into()))?;
    // Cloud CSV stores content properties as CUSTOM content (pluginkey ...content-property) whose
    // `pageid` is the owner, `title` the property name and the body (bodytypeid 1) the JSON value.
    let mut property_rows: HashMap<String, (String, String)> = HashMap::new(); // property content id → (owner, name)
    for_each_row(source, &content_file, |r| {
        let kind = ContentKind::from_csv_type(&get(&r, "contenttype").unwrap_or_default());
        if kind == ContentKind::Custom && get(&r, "pluginkey").is_some_and(|k| k.ends_with(":content-property")) {
            if let (Some(id), Some(owner), Some(name)) = (get(&r, "contentid"), get(&r, "pageid"), get(&r, "title"))
                && get(&r, "prevver").is_none()
            {
                property_rows.insert(id, (owner, name));
            }
            return;
        }
        let id = get(&r, "contentid").unwrap_or_default();
        let is_comment = kind == ContentKind::Comment;
        let is_attachment = kind == ContentKind::Attachment;
        let page_id = get(&r, "pageid");
        let c = Content {
            id: id.clone(),
            kind,
            title: get(&r, "title").unwrap_or_default(),
            status: ContentStatus::parse(&get(&r, "content_status").unwrap_or_else(|| "current".into())),
            version: get_i64(&r, "version").unwrap_or(1),
            original_version_id: get(&r, "prevver"),
            parent_id: if is_comment || is_attachment { None } else { get(&r, "parentid") },
            position: get_i64(&r, "child_position"),
            space_id: get(&r, "spaceid"),
            container_id: if is_comment || is_attachment { page_id.or_else(|| get(&r, "parentccid")) } else { None },
            parent_comment_id: get(&r, "parentcommentid"),
            creator: get(&r, "creator"),
            created: get(&r, "creationdate"),
            last_modifier: get(&r, "lastmodifier"),
            modified: get(&r, "lastmoddate"),
            version_comment: get(&r, "versioncomment"),
            plugin_key: get(&r, "pluginkey"),
            property_ids: vec![],
        };
        export.contents.insert(id, c);
    })?;

    if let Some(n) = find(source, &names, "bodycontent") {
        for_each_row(source, &n, |r| {
            if let Some(cid) = get(&r, "contentid") {
                if let Some((owner, name)) = property_rows.get(&cid) {
                    let value = r.get("body").cloned().unwrap_or_default();
                    export.properties.entry(owner.clone()).or_default().insert(name.clone(), value);
                    return;
                }
                let body = r.get("body").cloned().unwrap_or_default();
                export.add_body(Body { content_id: cid, kind: BodyKind::guess(get(&r, "bodytypeid").as_deref(), &body), body });
            }
        })?;
    } else {
        export.read_warnings.push("bodycontent.csv not found".into());
    }

    // Users: export-users.csv ("User id","User name") + optional user_mapping
    if let Some(n) = names.iter().find(|n| n.rsplit('/').next().is_some_and(|b| b.eq_ignore_ascii_case("export-users.csv"))).cloned() {
        for_each_row(source, &n, |r| {
            if let Some(key) = get(&r, "user id") {
                let u = export.users.entry(key.clone()).or_insert_with(|| User { key: key.clone(), ..Default::default() });
                u.display_name = get(&r, "user name").or(u.display_name.take());
                u.email = get(&r, "email").or(u.email.take());
                if key.contains(':') || key.len() == 24 {
                    u.account_id.get_or_insert(key);
                }
            }
        })?;
    }
    if let Some(n) = find(source, &names, "user_mapping") {
        for_each_row(source, &n, |r| {
            if let Some(key) = get(&r, "user_key") {
                let u = export.users.entry(key.clone()).or_insert_with(|| User { key, ..Default::default() });
                u.username = get(&r, "username").or(u.username.take());
            }
        })?;
    }

    // Content properties (MEDIA_TYPE, FILESIZE, inline comment anchors, ...)
    if let Some(n) = find(source, &names, "contentproperties") {
        for_each_row(source, &n, |r| {
            if let (Some(cid), Some(name)) = (get(&r, "contentid"), get(&r, "propertyname")) {
                let v = get(&r, "stringval").or_else(|| get(&r, "longval")).or_else(|| get(&r, "dateval")).unwrap_or_default();
                export.properties.entry(cid).or_default().insert(name, v);
            }
        })?;
    }

    // Labels
    if let (Some(ln), Some(cln)) = (find(source, &names, "label"), find(source, &names, "content_label")) {
        let mut labels: HashMap<String, (String, String)> = HashMap::new();
        for_each_row(source, &ln, |r| {
            if let Some(id) = get(&r, "labelid") {
                labels.insert(id, (get(&r, "name").unwrap_or_default(), get(&r, "namespace").unwrap_or_else(|| "global".into())));
            }
        })?;
        for_each_row(source, &cln, |r| {
            if let (Some(lid), Some(cid)) = (get(&r, "labelid"), get(&r, "contentid"))
                && let Some((name, ns)) = labels.get(&lid)
            {
                export.labellings.push(Labelling { content_id: cid, name: name.clone(), namespace: ns.clone(), owner: get(&r, "owner") });
            }
        })?;
    }

    // Space permissions
    if let Some(n) = find(source, &names, "spacepermissions") {
        for_each_row(source, &n, |r| {
            if inactive(get(&r, "active").as_deref()) {
                return;
            }
            export.space_permissions.push(SpacePermission::new(
                get(&r, "spaceid").unwrap_or_default(),
                get(&r, "permtype").unwrap_or_default(),
                get(&r, "permusername"),
                get(&r, "permgroupname"),
                get(&r, "permalluserssubject").as_deref(),
            ));
        })?;
    }

    // Page restrictions
    if let (Some(sn), Some(pn)) = (find(source, &names, "content_perm_set"), find(source, &names, "content_perm")) {
        let mut sets: HashMap<String, (String, RestrictionKind)> = HashMap::new();
        for_each_row(source, &sn, |r| {
            if let (Some(id), Some(cid)) = (get(&r, "id"), get(&r, "content_id")) {
                let kind = if get(&r, "cont_perm_type").unwrap_or_default().to_ascii_lowercase().contains("edit") {
                    RestrictionKind::Edit
                } else {
                    RestrictionKind::View
                };
                sets.insert(id, (cid, kind));
            }
        })?;
        for_each_row(source, &pn, |r| {
            if let Some(set) = get(&r, "cps_id")
                && let Some((cid, kind)) = sets.get(&set)
            {
                export.restrictions.push(Restriction { content_id: cid.clone(), kind: *kind, user_key: get(&r, "username"), group: get(&r, "groupname") });
            }
        })?;
    }

    // Watches (NOTIFICATIONS: USERNAME holds the user key) and favourites (USERCONTENT_RELATION)
    if let Some(n) = find(source, &names, "notifications") {
        for_each_row(source, &n, |r| {
            let flag = |k: &str| get(&r, k).is_some_and(|v| v != "0" && v != "f" && v != "false");
            let Some(user) = get(&r, "username") else { return };
            if flag("digest") || flag("network") || get(&r, "labelid").is_some() {
                return;
            }
            match (get(&r, "contentid"), get(&r, "spaceid")) {
                (Some(c), _) => export.watches.push(Watch { user_key: user, content_id: Some(c), space_id: None }),
                (None, Some(s)) if get(&r, "contenttype").is_none_or(|t| t.eq_ignore_ascii_case("page")) => {
                    export.watches.push(Watch { user_key: user, content_id: None, space_id: Some(s) })
                }
                _ => {}
            }
        })?;
    }
    if let Some(n) = find(source, &names, "usercontent_relation") {
        for_each_row(source, &n, |r| {
            if get(&r, "relationname").is_some_and(|v| v.eq_ignore_ascii_case("favourite") || v.eq_ignore_ascii_case("favorite"))
                && let (Some(user), Some(c)) = (get(&r, "sourceuser"), get(&r, "targetcontentid"))
            {
                export.favourites.push(Favourite { user_key: user, content_id: c });
            }
        })?;
    }

    crate::xml_export::index_attachments(&names, export);
    Ok(())
}
