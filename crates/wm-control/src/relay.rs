//! In-process relay routing table and explicit session cancellation.
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;
use wm_proto::{NodeId, RelayFrame};

struct RelayConn {
    conn_id: Uuid,
    tx: mpsc::Sender<Vec<u8>>,
    cancel: watch::Sender<bool>,
}

#[derive(Default)]
pub struct RelayHub {
    // One lock makes source validation + forwarding atomic with disconnect/replacement.
    conns: Mutex<HashMap<NodeId, RelayConn>>,
}

impl RelayHub {
    pub fn new() -> Self { Self::default() }

    pub fn register(&self, node: NodeId, tx: mpsc::Sender<Vec<u8>>) -> Uuid {
        let conn_id = Uuid::new_v4();
        let (cancel, _) = watch::channel(false);
        if let Some(old) = self.conns.lock().unwrap().insert(node, RelayConn { conn_id, tx, cancel }) {
            old.cancel.send_replace(true);
        }
        conn_id
    }

    pub fn cancellation(&self, node: NodeId, conn_id: Uuid) -> Option<watch::Receiver<bool>> {
        self.conns.lock().unwrap().get(&node)
            .filter(|conn| conn.conn_id == conn_id).map(|conn| conn.cancel.subscribe())
    }

    /// Only the current session may remove itself or announce offline.
    pub fn unregister(&self, node: NodeId, conn_id: Uuid) -> bool {
        let mut conns = self.conns.lock().unwrap();
        if conns.get(&node).is_some_and(|conn| conn.conn_id == conn_id) {
            if let Some(conn) = conns.remove(&node) { conn.cancel.send_replace(true); }
            Self::presence(&conns, node, false);
            true
        } else { false }
    }

    pub fn disconnect(&self, node: NodeId) {
        let mut conns = self.conns.lock().unwrap();
        if let Some(conn) = conns.remove(&node) {
            conn.cancel.send_replace(true);
            Self::presence(&conns, node, false);
        }
    }

    fn presence(conns: &HashMap<NodeId, RelayConn>, node: NodeId, online: bool) {
        if let Ok(bytes) = (RelayFrame::Presence { node, online }).encode() {
            for conn in conns.values() { let _ = conn.tx.try_send(bytes.clone()); }
        }
    }

    pub fn announce_online(&self, node: NodeId, conn_id: Uuid) {
        let conns = self.conns.lock().unwrap();
        if conns.get(&node).is_some_and(|conn| conn.conn_id == conn_id) {
            Self::presence(&conns, node, true);
        }
    }

    pub fn is_online(&self, node: NodeId) -> bool {
        self.conns.lock().unwrap().contains_key(&node)
    }

    /// Stale/revoked readers cannot forward, even when a frame raced cancellation.
    pub fn send_from(&self, src: NodeId, conn_id: Uuid, dst: NodeId, frame: &RelayFrame) -> bool {
        let Ok(bytes) = frame.encode() else { return false; };
        let conns = self.conns.lock().unwrap();
        if !conns.get(&src).is_some_and(|conn| conn.conn_id == conn_id) { return false; }
        conns.get(&dst).is_some_and(|conn| conn.tx.try_send(bytes).is_ok())
    }

    pub fn send(&self, dst: NodeId, frame: &RelayFrame) -> bool {
        let Ok(bytes) = frame.encode() else { return false; };
        self.conns.lock().unwrap().get(&dst).is_some_and(|conn| conn.tx.try_send(bytes).is_ok())
    }

    pub fn broadcast(&self, frame: &RelayFrame) {
        let Ok(bytes) = frame.encode() else { return; };
        for conn in self.conns.lock().unwrap().values() { let _ = conn.tx.try_send(bytes.clone()); }
    }
    pub fn online_count(&self) -> usize { self.conns.lock().unwrap().len() }
    pub fn online_nodes(&self) -> Vec<NodeId> { self.conns.lock().unwrap().keys().copied().collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replacement_cancels_old_session_without_removing_new_one() {
        let hub = RelayHub::new();
        let node = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        let old = hub.register(node, tx);
        let mut cancel = hub.cancellation(node, old).unwrap();
        let (tx, _rx) = mpsc::channel(8);
        let new = hub.register(node, tx);
        cancel.changed().await.unwrap();
        assert!(*cancel.borrow());
        assert!(!hub.unregister(node, old));
        assert!(hub.cancellation(node, new).is_some());
        assert!(!hub.send_from(node, old, node, &RelayFrame::Ping));
    }

    #[tokio::test]
    async fn revoke_cancels_and_rejects_frames_even_with_sender_alive() {
        let hub = RelayHub::new();
        let node = Uuid::new_v4();
        let dst = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        let id = hub.register(node, tx.clone());
        let mut cancel = hub.cancellation(node, id).unwrap();
        let (dst_tx, mut dst_rx) = mpsc::channel(8);
        hub.register(dst, dst_tx);
        assert!(hub.send_from(node, id, dst, &RelayFrame::Ping));
        dst_rx.recv().await.unwrap();
        hub.disconnect(node);
        cancel.changed().await.unwrap();
        assert!(*cancel.borrow());
        for _ in 0..100 { assert!(!hub.send_from(node, id, dst, &RelayFrame::Ping)); }
        let presence = dst_rx.try_recv().unwrap();
        assert!(matches!(RelayFrame::decode(&presence), Ok(RelayFrame::Presence { node: n, online: false }) if n == node));
        assert!(dst_rx.try_recv().is_err());
    }

    #[test]
    fn disconnect_linearizes_against_concurrent_forwarding() {
        let hub = std::sync::Arc::new(RelayHub::new());
        let node = Uuid::new_v4();
        let dst = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(8);
        let id = hub.register(node, tx);
        let (tx, _rx) = mpsc::channel(4096);
        hub.register(dst, tx);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let thread = {
            let hub = hub.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..1000 { hub.send_from(node, id, dst, &RelayFrame::Ping); }
            })
        };
        barrier.wait();
        hub.disconnect(node);
        for _ in 0..1000 { assert!(!hub.send_from(node, id, dst, &RelayFrame::Ping)); }
        thread.join().unwrap();
    }

}
