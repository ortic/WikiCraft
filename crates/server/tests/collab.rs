mod common;

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use wikicraft_doc_model::{Node, ydoc};
use yrs::sync::{Message, SyncMessage};
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ReadTxn, Text, Transact, Update};

type Stream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Peer {
    ws: Stream,
    doc: yrs::Doc,
}

impl Peer {
    async fn connect(base: &str, page: &str, token: &str) -> Peer {
        let url = format!("{}/api/pages/{page}/collab", base.replace("http://", "ws://"));
        let mut req = url.into_client_request().unwrap();
        req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
        let (ws, _) = tokio_tungstenite::connect_async(req).await.expect("ws connect");
        let mut p = Peer { ws, doc: ydoc::new_doc() };
        // ask for the server state
        let sv = p.doc.transact().state_vector();
        p.send(Message::Sync(SyncMessage::SyncStep1(sv))).await;
        p.pump_until(|m| matches!(m, Message::Sync(SyncMessage::SyncStep2(_)))).await;
        p
    }

    async fn send(&mut self, m: Message) {
        self.ws.send(Ws::Binary(m.encode_v1().into())).await.unwrap();
    }

    /// Processes incoming messages until one matches `stop` (applies updates along the way).
    async fn pump_until(&mut self, stop: impl Fn(&Message) -> bool) {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), self.ws.next()).await.expect("message in time").expect("stream open").unwrap();
            let Ws::Binary(data) = msg else { continue };
            let m = Message::decode_v1(&data).unwrap();
            match &m {
                Message::Sync(SyncMessage::SyncStep2(u)) | Message::Sync(SyncMessage::Update(u)) => {
                    self.doc.transact_mut().apply_update(Update::decode_v1(u).unwrap()).unwrap();
                }
                Message::Sync(SyncMessage::SyncStep1(sv)) => {
                    let u = self.doc.transact().encode_state_as_update_v1(sv);
                    self.send(Message::Sync(SyncMessage::SyncStep2(u))).await;
                }
                _ => {}
            }
            if stop(&m) {
                return;
            }
        }
    }

    /// Appends a paragraph and sends the resulting update.
    async fn append_paragraph(&mut self, text: &str) {
        let before = self.doc.transact().state_vector();
        {
            let frag = self.doc.get_or_insert_xml_fragment(ydoc::FIELD);
            let mut txn = self.doc.transact_mut();
            let p = frag.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
            let t = yrs::types::xml::XmlFragment::push_back(&p, &mut txn, XmlTextPrelim::new(""));
            t.insert(&mut txn, 0, text);
        }
        let update = self.doc.transact().encode_state_as_update_v1(&before);
        self.send(Message::Sync(SyncMessage::Update(update))).await;
    }

    fn text(&self) -> String {
        wikicraft_doc_model::text::plain_text(&ydoc::doc_to_pm(&self.doc))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn live_collaboration_roundtrip() {
    let app = common::spawn().await;
    let owner = app.setup().await;
    let (_, space) = app.post(&owner, "/spaces", json!({"key": "LIVE", "name": "Live", "defaultRole": "viewer"})).await;
    let page = space["homePageId"].as_str().unwrap().to_string();
    let (_, t) = app.post(&owner, "/me/tokens", json!({"name": "ws"})).await;
    let token = t["token"].as_str().unwrap().to_string();

    let mut a = Peer::connect(&app.base, &page, &token).await;
    let mut b = Peer::connect(&app.base, &page, &token).await;
    // server seeded the Yjs doc from the stored content (welcome text)
    assert!(a.text().contains("Welcome to the Live space"), "{}", a.text());
    assert_eq!(a.text(), b.text());

    a.append_paragraph("Hello from A — grüße 👋").await;
    b.pump_until(|m| matches!(m, Message::Sync(SyncMessage::Update(_)))).await; // the first update B sees must be A's edit
    assert!(b.text().contains("Hello from A — grüße 👋"));

    b.append_paragraph("Reply from B").await;
    a.pump_until(|m| matches!(m, Message::Sync(SyncMessage::Update(_)))).await;
    assert_eq!(a.text(), b.text());

    // a viewer can watch but not write
    let (viewer, _) = app.user(&owner, "viewer@acme.test", "member").await;
    let (_, vt) = app.post(&viewer, "/me/tokens", json!({"name": "ws"})).await;
    let mut v = Peer::connect(&app.base, &page, vt["token"].as_str().unwrap()).await;
    assert!(v.text().contains("Reply from B"));
    v.append_paragraph("viewer vandalism").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // REST content is the live content while the room is open
    let (_, p) = app.get(&owner, &format!("/pages/{page}")).await;
    let content = p["content"].to_string();
    assert!(content.contains("Reply from B"));
    assert!(!content.contains("vandalism"), "read-only updates must be ignored");

    // everyone leaves → persisted to the DB with a new version
    drop(a);
    drop(b);
    drop(v);
    let stored = app
        .eventually(|| async {
            let row: (serde_json::Value, i32) =
                sqlx::query_as("SELECT content, version FROM pages WHERE id = $1::uuid").bind(&page).fetch_one(&app.state.db).await.unwrap();
            (row.0.to_string().contains("Reply from B") && row.1 >= 2).then_some(row)
        })
        .await;
    let doc = Node::from_json(&stored.0).unwrap();
    assert!(wikicraft_doc_model::text::plain_text(&doc).contains("grüße 👋"));

    // a REST update reaches a connected editor live
    let mut c = Peer::connect(&app.base, &page, &token).await;
    let (st, _) = app.put(&owner, &format!("/pages/{page}/content"), json!({"markdown": "Replaced via API"})).await;
    assert_eq!(st, 200);
    c.pump_until(|m| matches!(m, Message::Sync(SyncMessage::Update(_)))).await;
    assert_eq!(c.text(), "Replaced via API");
}
