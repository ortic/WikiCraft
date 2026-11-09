//! A minimal y-websocket client (what the browser editor speaks) for tests.

#![allow(dead_code)]

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use wikicraft_doc_model::ydoc;
use yrs::sync::{Message, SyncMessage};
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ReadTxn, Text, Transact, Update};

type Stream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub struct Peer {
    pub ws: Stream,
    pub doc: yrs::Doc,
}

impl Peer {
    pub async fn connect(base: &str, page: &str, token: &str) -> Peer {
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

    pub async fn send(&mut self, m: Message) {
        self.ws.send(Ws::Binary(m.encode_v1().into())).await.unwrap();
    }

    /// Processes incoming messages until one matches `stop` (applies updates along the way).
    pub async fn pump_until(&mut self, stop: impl Fn(&Message) -> bool) {
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
    pub async fn append_paragraph(&mut self, text: &str) {
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

    pub fn text(&self) -> String {
        wikicraft_doc_model::text::plain_text(&ydoc::doc_to_pm(&self.doc))
    }
}

impl Peer {
    /// Waits until the document text contains `needle`.
    pub async fn wait_for_text(&mut self, needle: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !self.text().contains(needle) {
            assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {needle:?}; have {:?}", self.text());
            let _ = tokio::time::timeout(Duration::from_millis(500), self.pump_until(|m| matches!(m, Message::Sync(SyncMessage::Update(_))))).await;
        }
    }
}
