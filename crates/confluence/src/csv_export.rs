//! Reader for Confluence Cloud CSV space exports (`exportFormat=csv`).
//!
//! Verified against a real export (Oct 2026): `spaces.csv.gz`, `content.csv.gz`,
//! `bodycontent.csv.gz`, `export-users.csv`; and against the two complete exports of the XWiki
//! test corpus (`tests/xwiki_corpus.rs`): `label`, `content_label`, `notifications`,
//! `usercontent_relation`, `spacepermissions`, `content_perm_set` / `content_perm`,
//! `contentproperties`, `user_mapping` use the Confluence database column names read here.
//!
//! Site backups (Settings → Backup and restore, Nov 2026: `metadata.json`, `data/*.csv.gz`,
//! `attachments/<FILESTORE_ID>`) differ: bodies are zstd + base64, there are no user names or
//! e-mail addresses (`user_mapping` only has account ids), and newer permission tables
//! (`content_permissions`, `spacerole_assignments`) may appear next to the classic ones.

use std::collections::{HashMap, HashSet};
use std::io::Read;

use base64::Engine;

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

/// Uncompressed size above which a compressed body is refused (zip-bomb guard).
const MAX_BODY_BYTES: u64 = 256 * 1024 * 1024;

/// Cloud site backups store `bodycontent.body` zstd-compressed and base64-encoded (`KLUv/…`, the
/// zstd magic); space exports store the text as is. Text that only starts like that stays text.
fn decode_body(body: String) -> Result<String, String> {
    if !body.starts_with("KLUv/") {
        return Ok(body);
    }
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    let Ok(raw) = engine.decode(body.trim()) else { return Ok(body) };
    let mut out = Vec::new();
    zstd::stream::read::Decoder::new(raw.as_slice()).and_then(|d| d.take(MAX_BODY_BYTES + 1).read_to_end(&mut out)).map_err(|e| format!("zstd: {e}"))?;
    if out.len() as u64 > MAX_BODY_BYTES {
        return Err("larger than 256 MiB".into());
    }
    Ok(String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
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
        let mut undecodable: Vec<String> = Vec::new();
        for_each_row(source, &n, |mut r| {
            if let Some(cid) = get(&r, "contentid") {
                let body = match decode_body(r.remove("body").unwrap_or_default()) {
                    Ok(b) => b,
                    Err(e) => {
                        undecodable.push(format!("{cid} ({e})"));
                        return;
                    }
                };
                if let Some((owner, name)) = property_rows.get(&cid) {
                    export.properties.entry(owner.clone()).or_default().insert(name.clone(), body);
                    return;
                }
                export.add_body(Body { content_id: cid, kind: BodyKind::guess(get(&r, "bodytypeid").as_deref(), &body), body });
            }
        })?;
        if !undecodable.is_empty() {
            let n = undecodable.len();
            undecodable.truncate(5);
            export.read_warnings.push(format!("{n} compressed bodies could not be decompressed and stay empty, e.g. content {}", undecodable.join(", ")));
        }
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
                let aaid = get(&r, "aaid");
                // Cloud has no usernames: the column repeats the account id there
                let username = get(&r, "username").filter(|n| Some(n) != aaid.as_ref());
                u.username = username.or(u.username.take());
                if let Some(a) = aaid {
                    u.account_id.get_or_insert(a);
                }
            }
        })?;
    }
    // Site backups carry no user names (and no e-mail addresses), but personal spaces are named
    // after their owners
    let mut owners: HashMap<String, String> = HashMap::new(); // space id → owner's user key
    if let Some(n) = find(source, &names, "space_owner") {
        for_each_row(source, &n, |r| {
            if get(&r, "ownertype").is_none_or(|t| t.eq_ignore_ascii_case("user"))
                && let (Some(space), Some(owner)) = (get(&r, "spaceid"), get(&r, "ownerid"))
            {
                owners.insert(space, owner);
            }
        })?;
    }
    for s in export.spaces.iter().filter(|s| s.is_personal() && !s.name.trim().is_empty()) {
        let Some(owner) = owners.get(&s.id).or(s.creator.as_ref()) else { continue };
        let u = export.users.entry(owner.clone()).or_insert_with(|| User { key: owner.clone(), ..Default::default() });
        u.display_name.get_or_insert_with(|| s.name.trim().to_string());
    }
    let unnamed = export.users.values().filter(|u| u.display_name.is_none() && u.email.is_none() && u.username.is_none()).count();
    if unnamed > 0 {
        export.read_warnings.push(format!(
            "users without name: {unnamed} of {} imported authors get a placeholder name (site backups have no names or e-mail addresses; people with a personal space are named after it). Merge them into their accounts: Administration → Users → Merge into…",
            export.users.len()
        ));
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
    // Cloud group id → name (newer tables name groups by id only)
    let mut group_names: HashMap<String, String> = HashMap::new();
    if let Some(n) = find(source, &names, "spacepermissions") {
        for_each_row(source, &n, |r| {
            if let (Some(id), Some(name)) = (get(&r, "externalgroupid"), get(&r, "permgroupname")) {
                group_names.insert(id, name);
            }
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
            if let (Some(id), Some(name)) = (get(&r, "external_group_id"), get(&r, "groupname")) {
                group_names.insert(id, name);
            }
            if let Some(set) = get(&r, "cps_id")
                && let Some((cid, kind)) = sets.get(&set)
            {
                export.restrictions.push(Restriction { content_id: cid.clone(), kind: *kind, user_key: get(&r, "username"), group: get(&r, "groupname") });
            }
        })?;
    }
    // Newer Cloud sites keep page permissions in CONTENT_PERMISSIONS, by principal id. A
    // restriction must not get lost (the page would open up): entries that can't be told apart
    // restrict the page to the importing admin, and the report asks to review them.
    if let Some(n) = find(source, &names, "content_permissions") {
        let restricted: HashSet<String> = export.restrictions.iter().map(|r| r.content_id.clone()).collect();
        let (mut taken, mut unclear) = (0usize, 0usize);
        for_each_row(source, &n, |r| {
            let Some(cid) = get(&r, "content_id") else { return };
            if get(&r, "deleted_id").is_some() || restricted.contains(&cid) {
                return;
            }
            let ty = get(&r, "permission_type").unwrap_or_default().to_ascii_lowercase();
            let kind = if ["edit", "update", "write"].iter().any(|k| ty.contains(k)) { RestrictionKind::Edit } else { RestrictionKind::View };
            let principal = get(&r, "principal_id");
            let (user_key, group) = match get(&r, "principal_type").map(|t| t.to_ascii_lowercase()).as_deref() {
                Some("user") if principal.is_some() => (principal, None),
                Some("group") if principal.is_some() => (None, principal.map(|g| group_names.get(&g).cloned().unwrap_or(g))),
                _ => (None, None),
            };
            if (user_key.is_none() && group.is_none()) || !["view", "read", "edit", "update", "write"].iter().any(|k| ty.contains(k)) {
                unclear += 1;
            }
            taken += 1;
            export.restrictions.push(Restriction { content_id: cid, kind, user_key, group });
        })?;
        if taken > 0 {
            export.read_warnings.push(format!(
                "{taken} page permissions came from content_permissions ({unclear} of unknown kind or subject, restricted to the importing admin): please review the restricted pages"
            ));
        }
    }
    if let Some(n) = find(source, &names, "spacerole_assignments") {
        let mut roles = 0;
        for_each_row(source, &n, |r| {
            if get(&r, "deleted_id").is_none() && get(&r, "principal_id").is_some() {
                roles += 1;
            }
        })?;
        if roles > 0 {
            export
                .read_warnings
                .push(format!("{roles} space role assignments are not imported (only classic space permissions): please review the space permissions"));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(text: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(zstd::stream::encode_all(text.as_bytes(), 3).unwrap())
    }

    fn site_backup(tables: &[(&str, String)]) -> Export {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("data")).unwrap();
        std::fs::write(dir.path().join("metadata.json"), r#"{"transferType":"BACKUP"}"#).unwrap();
        for (name, csv) in tables {
            std::fs::write(dir.path().join("data").join(format!("{name}.csv")), csv).unwrap();
        }
        crate::read(&Source::open(dir.path()).unwrap()).unwrap()
    }

    fn tables() -> Vec<(&'static str, String)> {
        vec![
            ("spaces", "spaceid,spacename,spacekey,homepage,creator,spacetype,spacestatus\n1,Docs,DOC,10,k1,global,CURRENT\n2,Ada Lovelace,~5570581234,20,k2,personal,CURRENT\n".into()),
            (
                "content",
                "contentid,contenttype,title,version,creator,prevver,content_status,pageid,spaceid,parentid,pluginkey\n10,PAGE,Home,1,k1,,current,,1,,\n11,PAGE,Secret,1,k1,,current,,1,10,\n20,PAGE,Ada's home,1,k2,,current,,2,,\n30,CUSTOM,editor,1,k1,,current,10,1,,com.atlassian.confluence.content.property:content-property\n".into(),
            ),
            (
                "bodycontent",
                format!(
                    "bodycontentid,body,contentid,bodytypeid\n100,{},10,2\n101,{},11,2\n102,KLUv/ is how it starts,20,2\n103,{},30,1\n",
                    packed("<p>Hello <ac:link><ri:user ri:account-id=\"557058:ada\" /></ac:link></p>"),
                    packed("<p>for few eyes</p>"),
                    packed("\"v2\""),
                ),
            ),
            ("user_mapping", "user_key,username,lower_username,aaid\nk1,557058:bob,557058:bob,557058:bob\nk2,557058:ada,557058:ada,557058:ada\nk3,addon_bot,addon_bot,5cb4\n".into()),
            ("space_owner", "id,spaceid,ownerid,ownertype\n1,2,k2,USER\n".into()),
            ("spacepermissions", "permid,spaceid,permtype,permgroupname,externalgroupid,permusername,permalluserssubject,active\n1,1,VIEWSPACE,team,g-1,,,t\n".into()),
            (
                "content_permissions",
                "permission_id,permission_type,content_id,principal_type,principal_id,deleted_id\n1,VIEW,11,GROUP,g-1,\n2,EDIT,11,USER,557058:bob,\n3,VIEW,10,USER,557058:bob,7\n4,VIEW,20,TEAM,t-9,\n".into(),
            ),
        ]
    }

    #[test]
    fn site_backup_bodies_are_decompressed() {
        let ex = site_backup(&tables());
        let home = ex.body("10").unwrap();
        assert_eq!(home.kind, BodyKind::Storage);
        assert!(home.body.starts_with("<p>Hello <ac:link>"), "{}", home.body);
        assert_eq!(ex.body("11").unwrap().body, "<p>for few eyes</p>");
        // text that merely starts like a compressed body stays as it is
        assert_eq!(ex.body("20").unwrap().body, "KLUv/ is how it starts");
        // content properties are stored as compressed bodies, too
        assert_eq!(ex.property("10", "editor"), Some("\"v2\""));
        assert!(ex.read_warnings.iter().all(|w| !w.contains("decompress")), "{:?}", ex.read_warnings);
    }

    #[test]
    fn broken_compressed_bodies_are_reported() {
        let mut t = tables();
        t[2].1 = "bodycontentid,body,contentid,bodytypeid\n100,KLUv/SAzmQEAQ29u,10,2\n".into();
        let ex = site_backup(&t);
        assert!(ex.body("10").is_none(), "no gibberish page");
        assert!(
            ex.read_warnings.iter().any(|w| w.contains("1 compressed bodies could not be decompressed") && w.contains("content 10")),
            "{:?}",
            ex.read_warnings
        );
    }

    #[test]
    fn site_backup_users_have_account_ids_and_owner_names() {
        let ex = site_backup(&tables());
        let bob = &ex.users["k1"];
        assert_eq!(bob.account_id.as_deref(), Some("557058:bob"));
        assert_eq!(bob.username, None, "an account id is no username (it would become the name)");
        assert_eq!(bob.display_name, None);
        let ada = &ex.users["k2"];
        assert_eq!(ada.display_name.as_deref(), Some("Ada Lovelace"), "named after her personal space");
        assert_eq!(ex.users["k3"].username.as_deref(), Some("addon_bot"));
        assert_eq!(ex.users["k3"].account_id.as_deref(), Some("5cb4"));
        assert!(ex.read_warnings.iter().any(|w| w.starts_with("users without name: 1 of 3 imported authors get a placeholder name")), "{:?}", ex.read_warnings);
    }

    #[test]
    fn content_permissions_become_restrictions() {
        let ex = site_backup(&tables());
        let of = |cid: &str| ex.restrictions.iter().filter(|r| r.content_id == cid).map(|r| (r.kind, r.user_key.clone(), r.group.clone())).collect::<Vec<_>>();
        assert_eq!(
            of("11"),
            vec![(RestrictionKind::View, None, Some("team".into())), (RestrictionKind::Edit, Some("557058:bob".into()), None)],
            "group ids are named through the space permissions"
        );
        assert!(of("10").is_empty(), "deleted entries are skipped");
        assert_eq!(of("20"), vec![(RestrictionKind::View, None, None)], "an unknown subject fails closed");
        assert!(ex.read_warnings.iter().any(|w| w.contains("3 page permissions came from content_permissions (1 of unknown")), "{:?}", ex.read_warnings);
    }
}
