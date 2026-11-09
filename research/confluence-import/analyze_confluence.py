"""Dry-run analysis of a Confluence space export (classic XML or Cloud CSV).

Usage:
  python3 -I analyze_confluence.py xml <export.zip>
  python3 -I analyze_confluence.py csv <dir-with-*.csv.gz>

Prints an inventory of entity types, body formats, macros, storage-format
elements, link/attachment/user references and referential-integrity problems.
Only aggregate counts are printed (no page bodies, no user data).
"""
import collections
import csv
import gzip
import html.entities
import io
import json
import re
import sys
import xml.etree.ElementTree as ET
import zipfile

csv.field_size_limit(1 << 30)
C = collections.Counter

RE_MACRO = re.compile(r'<ac:structured-macro\b[^>]*?\bac:name="([^"]+)"')
RE_MACRO_LEGACY = re.compile(r'<ac:macro\b[^>]*?\bac:name="([^"]+)"')
RE_ADF_EXT = re.compile(r'<ac:adf-extension>\s*<ac:adf-node\b[^>]*?\btype="([^"]+)"')
RE_ADF_NODE = re.compile(r'<ac:adf-node\b[^>]*?\btype="([^"]+)"')
RE_ADF_ATTR = re.compile(r'<ac:adf-attribute\b[^>]*?\bkey="([^"]+)"')
RE_NS_EL = re.compile(r'<(ac|ri):([a-zA-Z-]+)')
RE_HTML_EL = re.compile(r'<([a-zA-Z][a-zA-Z0-9]*)[\s>/]')
RE_PARAM = re.compile(r'<ac:structured-macro\b[^>]*?\bac:name="([^"]+)"(.*?)</ac:structured-macro>', re.S)
RE_PARAM_NAME = re.compile(r'<ac:parameter\b[^>]*?\bac:name="([^"]*)"')
RE_RI_PAGE = re.compile(r'<ri:page\b([^>]*)/?>')
RE_RI_ATT = re.compile(r'<ri:attachment\b([^>]*)>')
RE_RI_USER = re.compile(r'<ri:user\b([^>]*)/?>')
RE_ATTR = re.compile(r'([a-zA-Z:-]+)="([^"]*)"')
RE_ENTITY = re.compile(r'&([a-zA-Z][a-zA-Z0-9]*);')
RE_WIKI_MACRO = re.compile(r'\{([a-zA-Z][a-zA-Z0-9-]*)[:}]')
RE_STYLE = re.compile(r'\bstyle="')
RE_DATA_ATTR = re.compile(r'\b(data-[a-z-]+)=')
RE_LOCAL_ID = re.compile(r'\b(ac:local-id|local-id|ac:macro-id|ac:schema-version)=')

XML_ENTITIES = {"amp", "lt", "gt", "quot", "apos"}
NS_WRAP = ('<root xmlns:ac="http://atlassian.com/content" '
           'xmlns:ri="http://atlassian.com/resource/identifier" '
           'xmlns:at="http://atlassian.com/template">{}</root>')


def to_strict_xml(body):
    """Make a storage-format body strict-XML parseable:
    1. undo Confluence's CDATA-terminator escape (`]]>` inside CDATA is exported as `]] >`)
    2. replace HTML named entities with numeric refs
    3. wrap in a root element declaring the ac:/ri:/at: namespaces
    """
    body = body.replace("]] >", "]]>")
    def rep(m):
        name = m.group(1)
        if name in XML_ENTITIES:
            return m.group(0)
        cp = html.entities.name2codepoint.get(name)
        return f"&#{cp};" if cp is not None else m.group(0)
    return NS_WRAP.format(RE_ENTITY.sub(rep, body))


class Stats:
    def __init__(self):
        self.c = collections.defaultdict(C)
        self.notes = []

    def inc(self, group, key, n=1):
        self.c[group][key] += n

    def dump(self, top=60):
        for group in sorted(self.c):
            cnt = self.c[group]
            print(f"\n## {group}  (distinct={len(cnt)}, total={sum(cnt.values())})")
            for k, v in cnt.most_common(top):
                print(f"  {v:>7}  {k}")
        for n in self.notes:
            print("NOTE:", n)


def analyze_storage(st, body, owner_type):
    st.inc("body: owner content type", owner_type)
    for m in RE_MACRO.findall(body):
        st.inc("storage: structured-macro names", m)
    for m in RE_MACRO_LEGACY.findall(body):
        st.inc("storage: legacy ac:macro names", m)
    for m in RE_ADF_EXT.findall(body):
        st.inc("storage: ac:adf-extension node types", m)
    for m in RE_ADF_NODE.findall(body):
        st.inc("storage: ac:adf-node types (all)", m)
    for m in RE_ADF_ATTR.findall(body):
        st.inc("storage: ac:adf-attribute keys", m)
    for ns, el in RE_NS_EL.findall(body):
        st.inc("storage: ac:/ri: elements", f"{ns}:{el}")
    for el in RE_HTML_EL.findall(body):
        st.inc("storage: HTML elements", el.lower())
    for m in RE_DATA_ATTR.findall(body):
        st.inc("storage: data-* attributes", m)
    for m in RE_LOCAL_ID.findall(body):
        st.inc("storage: id/version attributes", m)
    if RE_STYLE.search(body):
        st.inc("storage: bodies with inline style=", "yes")
    for mname, inner in RE_PARAM.findall(body):
        for p in RE_PARAM_NAME.findall(inner):
            st.inc("storage: macro parameters (macro.param)", f"{mname}.{p}")
    for ent in RE_ENTITY.findall(body):
        if ent not in XML_ENTITIES:
            st.inc("storage: HTML named entities (need mapping)", ent)
    try:
        ET.fromstring(to_strict_xml(body))
        st.inc("storage: strict XML parse", "ok")
    except ET.ParseError as e:
        st.inc("storage: strict XML parse", "FAIL")
        st.inc("storage: strict XML parse errors", re.sub(r"line \d+, column \d+", "L,C", str(e)))


def check_refs(st, body, page_titles_by_space, own_space_key, att_names, users):
    for attrs in RE_RI_PAGE.findall(body):
        a = dict(RE_ATTR.findall(attrs))
        title = a.get("ri:content-title")
        space = a.get("ri:space-key", own_space_key)
        if "ri:content-id" in a:
            st.inc("refs: ri:page", "by content-id")
        elif title is None:
            st.inc("refs: ri:page", "no title/id")
        elif space != own_space_key:
            st.inc("refs: ri:page", "cross-space (unresolvable inside one space export)")
        elif title in page_titles_by_space.get(space, set()):
            st.inc("refs: ri:page", "resolved by title")
        else:
            st.inc("refs: ri:page", "BROKEN (title not in export)")
    for attrs in RE_RI_ATT.findall(body):
        a = dict(RE_ATTR.findall(attrs))
        fn = a.get("ri:filename")
        if fn is None:
            st.inc("refs: ri:attachment", "no filename")
        elif fn in att_names:
            st.inc("refs: ri:attachment", "resolved (filename exists in export)")
        else:
            st.inc("refs: ri:attachment", "UNRESOLVED filename")
    for attrs in RE_RI_USER.findall(body):
        a = dict(RE_ATTR.findall(attrs))
        key = a.get("ri:account-id") or a.get("ri:userkey") or a.get("ri:username")
        kind = ("account-id" if "ri:account-id" in a else
                "userkey" if "ri:userkey" in a else
                "username" if "ri:username" in a else "none")
        st.inc("refs: ri:user identifier kind", kind)
        if users is not None:
            st.inc("refs: ri:user", "resolved" if key in users else "UNRESOLVED")


def tree_depths(parent_of, ids):
    depth = C()
    orphans = 0
    cycles = 0
    for i in ids:
        d, cur, seen = 0, i, set()
        while parent_of.get(cur):
            if cur in seen:
                cycles += 1
                break
            seen.add(cur)
            p = parent_of[cur]
            if p not in ids:
                orphans += 1
                break
            cur, d = p, d + 1
        depth[d] += 1
    return depth, orphans, cycles


# --------------------------------------------------------------------------- CSV
def run_csv(path):
    st = Stats()
    def rows(name):
        with gzip.open(f"{path}/{name}.csv.gz", "rt", encoding="utf-8", newline="") as f:
            yield from csv.DictReader(f)

    spaces = list(rows("spaces"))
    space_key = {s["spaceid"]: s["spacekey"] for s in spaces}
    content = list(rows("content"))
    by_id = {r["contentid"]: r for r in content}
    for r in content:
        st.inc("content: contenttype", r["contenttype"])
        st.inc("content: contenttype x status", f'{r["contenttype"]} / {r["content_status"]}')
        if r["prevver"]:
            st.inc("content: historical versions (prevver set) by type", r["contenttype"])
        if r["pluginkey"]:
            st.inc("content: CUSTOM pluginkey", r["pluginkey"])
        if r["parentcommentid"]:
            st.inc("content: threaded comment replies", "has parentcommentid")
        if r["navigationtype"]:
            st.inc("content: navigationtype", r["navigationtype"])
        if r["draftpageid"] or r["drafttype"]:
            st.inc("content: draft rows", r["drafttype"] or "draftpageid")
        st.inc("content: creator id format",
               "atlassian account-id (5570xx:/712020:/hex)" if r["creator"] and (":" in r["creator"] or re.fullmatch(r"[0-9a-f]{24}", r["creator"])) else (r["creator"] and "other" or "empty"))

    current_pages = {i: r for i, r in by_id.items()
                     if r["contenttype"] in ("PAGE", "FOLDER") and r["content_status"] == "current" and not r["prevver"]}
    titles = collections.defaultdict(set)
    for r in current_pages.values():
        titles[space_key.get(r["spaceid"], "?")].add(r["title"])
    parent_of = {i: r["parentid"] for i, r in current_pages.items()}
    depth, orphans, cycles = tree_depths(parent_of, set(current_pages))
    for d, n in sorted(depth.items()):
        st.inc("tree: current PAGE/FOLDER depth", f"depth {d:02d}", n)
    st.inc("tree: integrity", "orphans (parent missing)", orphans)
    st.inc("tree: integrity", "cycles", cycles)
    st.inc("tree: integrity", "pages with child_position set",
           sum(1 for r in current_pages.values() if r["child_position"]))
    dup = C((space_key.get(r["spaceid"]), r["title"]) for r in current_pages.values())
    st.inc("tree: integrity", "duplicate titles within space", sum(1 for v in dup.values() if v > 1))

    att_names = {r["title"] for r in content if r["contenttype"] == "ATTACHMENT"}
    users = None
    try:
        with open(f"{path}/export-users.csv", encoding="utf-8", newline="") as f:
            users = {r["User id"] for r in csv.DictReader(f)}
        st.inc("users", "export-users.csv rows", len(users))
    except FileNotFoundError:
        st.notes.append("no export-users.csv")
    creators = {r["creator"] for r in content if r["creator"]}
    if users is not None:
        st.inc("users", "distinct creators", len(creators))
        st.inc("users", "creators missing from export-users.csv", len(creators - users))

    home_key = spaces[0]["spacekey"] if spaces else "?"
    for b in rows("bodycontent"):
        owner = by_id.get(b["contentid"])
        otype = owner["contenttype"] if owner else "MISSING-OWNER"
        st.inc("body: bodytypeid x owner type", f'{b["bodytypeid"]} / {otype}' + (f' ({owner["pluginkey"].split(".")[-1]})' if owner and owner["pluginkey"] else ""))
        if b["bodytypeid"] == "2":
            is_current = owner and owner["content_status"] == "current" and not owner["prevver"]
            if is_current:
                analyze_storage(st, b["body"], otype)
                check_refs(st, b["body"], titles, home_key, att_names, users)
            else:
                st.inc("body: skipped (non-current version)", otype)
        elif b["bodytypeid"] == "0":
            for m in RE_WIKI_MACRO.findall(b["body"]):
                st.inc("wiki-markup: macro names", m)
    st.dump()


# --------------------------------------------------------------------------- XML
def run_xml(zpath):
    st = Stats()
    z = zipfile.ZipFile(zpath)
    names = z.namelist()
    att_files = [n for n in names if n.startswith("attachments/")]
    st.inc("zip", "entries", len(names))
    st.inc("zip", "attachment binaries", len(att_files))
    for n in att_files:
        st.inc("zip: attachment path shape", "attachments/" + "/".join("<id>" for _ in n.split("/")[1:]))

    objs = []  # (class, id, props, collections)
    with z.open("entities.xml") as f:
        for ev, el in ET.iterparse(f, events=("end",)):
            if el.tag != "object":
                continue
            cls = el.get("class")
            oid = el.findtext("id")
            props = {}
            colls = {}
            for p in el.findall("property"):
                name = p.get("name")
                inner_id = p.findtext("id")
                props[name] = inner_id if inner_id is not None else (p.text or "")
                if p.get("class"):
                    props[name + "@class"] = p.get("class")
            for c in el.findall("collection"):
                colls[c.get("name")] = len(c.findall("element"))
            objs.append((cls, oid, props, colls))
            el.clear()

    for cls, oid, props, colls in objs:
        st.inc("entities: object classes", cls)
        for k in props:
            if not k.endswith("@class"):
                st.inc(f"entities: properties of {cls}", k)
        for k, n in colls.items():
            st.inc(f"entities: collections of {cls}", k, 1)

    by_cls = collections.defaultdict(dict)
    for cls, oid, props, colls in objs:
        by_cls[cls][oid] = props
    content_classes = {"Page", "BlogPost", "Comment", "Attachment", "CustomContentEntityObject", "SpaceDescription", "Draft"}
    content = {}
    for cls in content_classes:
        for oid, p in by_cls.get(cls, {}).items():
            content[oid] = (cls, p)
            st.inc("content: class x status", f'{cls} / {p.get("contentStatus", "?")}' + (" / historical" if p.get("originalVersion") else ""))
            if cls == "CustomContentEntityObject":
                st.inc("content: custom pluginModuleKey", p.get("pluginModuleKey", "?"))

    space_key = {oid: p.get("key") for oid, p in by_cls.get("Space", {}).items()}
    pages = {oid: p for oid, p in by_cls.get("Page", {}).items()
             if p.get("contentStatus") == "current" and not p.get("originalVersion")}
    titles = collections.defaultdict(set)
    for p in pages.values():
        titles[space_key.get(p.get("space"), "?")].add(p.get("title"))
    parent_of = {oid: p.get("parent") for oid, p in pages.items()}
    depth, orphans, cycles = tree_depths(parent_of, set(pages))
    for d, n in sorted(depth.items()):
        st.inc("tree: current Page depth", f"depth {d:02d}", n)
    st.inc("tree: integrity", "orphans (parent missing)", orphans)
    st.inc("tree: integrity", "cycles", cycles)
    st.inc("tree: integrity", "pages with position set", sum(1 for p in pages.values() if p.get("position")))

    atts = by_cls.get("Attachment", {})
    att_names = {p.get("title") for p in atts.values()}
    # attachments/<containerId>/<attachmentId>/<version>
    att_paths = set(att_files)
    missing_bin = 0
    for oid, p in atts.items():
        if p.get("originalVersion"):
            continue
        cont = p.get("containerContent") or p.get("content")
        ver = p.get("version", "1")
        if f"attachments/{cont}/{oid}/{ver}" not in att_paths:
            missing_bin += 1
    st.inc("attachments: integrity", "current attachments", sum(1 for p in atts.values() if not p.get("originalVersion")))
    st.inc("attachments: integrity", "binary missing at attachments/<container>/<id>/<version>", missing_bin)
    for p in atts.values():
        st.inc("attachments: media types", p.get("mediaType") or p.get("contentType") or "?")

    users = set(by_cls.get("ConfluenceUserImpl", {}).keys())
    st.inc("users", "ConfluenceUserImpl objects", len(users))
    home_key = next(iter(space_key.values()), "?")
    for oid, p in by_cls.get("BodyContent", {}).items():
        owner = content.get(p.get("content"))
        otype, op = owner if owner else ("MISSING-OWNER", {})
        btype = p.get("bodyType", "?")
        st.inc("body: bodyType x owner class", f"{btype} / {otype}")
        body = p.get("body", "")
        is_current = op.get("contentStatus") == "current" and not op.get("originalVersion")
        if btype == "2" and is_current:
            analyze_storage(st, body, otype)
            check_refs(st, body, titles, home_key, att_names, users)
        elif btype == "0" and is_current:
            for m in RE_WIKI_MACRO.findall(body):
                st.inc("wiki-markup: macro names", m)
    for cls in ("Labelling", "Label", "ContentProperty", "SpacePermission", "ContentPermission",
                "ContentPermissionSet", "OutgoingLink", "ReferralLink", "Notification", "LikeEntity",
                "PageTemplate", "User2ContentRelationEntity", "BucketPropertySetItem"):
        if cls in by_cls:
            st.inc("entities: notable classes present", cls, len(by_cls[cls]))
    for p in by_cls.get("Label", {}).values():
        st.inc("labels: namespace", p.get("namespace", "?"))
    for p in by_cls.get("SpacePermission", {}).values():
        st.inc("permissions: space permission types", p.get("type", "?"))
    for p in by_cls.get("ContentPermission", {}).values():
        st.inc("permissions: content permission types", p.get("type", "?"))
    for p in by_cls.get("ContentProperty", {}).values():
        st.inc("content properties: names", p.get("name", "?"))
    st.dump()


if __name__ == "__main__":
    mode, path = sys.argv[1], sys.argv[2]
    run_csv(path) if mode == "csv" else run_xml(path)
