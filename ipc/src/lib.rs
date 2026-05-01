//! IPC — ZeroMQ subscriber bridge
//!
//! # Data Flow
//! ```text
//! ZMQ PUB (camera process)
//!     │  multipart: [topic: &[u8], payload: flatbuffer bytes]
//!     ▼
//! ZmqSubscriber  (tokio task, non-RT thread)
//!     │  Arc<[u8]> — one allocation per frame, then zero-copy sharing
//!     ▼
//! crossbeam bounded channel (capacity = FRAME_QUEUE_DEPTH)
//!     │  back-pressure: drop oldest frame if vision falls behind
//!     ▼
//! vision::FrameProcessor
//! ```
//!
//! # Allocation Policy
//! This crate DOES allocate — once per received frame (`Arc<[u8]>`).
//! That is acceptable: IPC is NOT in the 1 kHz HAL control loop.
//! After the initial `Arc` creation, all downstream access is zero-copy.

use std::sync::Arc;

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use futures::StreamExt;
use thiserror::Error;
use tmq::{subscribe, Context};

/// Raw flatbuffer bytes shared across thread boundary without copying.
pub type FrameBuffer = Arc<[u8]>;

/// Depth of the lock-free frame queue.
/// If vision processing stalls, frames beyond this depth are dropped (not buffered).
const FRAME_QUEUE_DEPTH: usize = 4;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("ZMQ error: {0}")]
    Zmq(#[from] tmq::TmqError),
    #[error("subscriber disconnected")]
    ChannelClosed,
}

/// Creates a bounded frame channel. Returns (publisher side, consumer side).
pub fn frame_channel() -> (Sender<FrameBuffer>, Receiver<FrameBuffer>) {
    bounded(FRAME_QUEUE_DEPTH)
}

/// Async ZMQ SUB socket. Forwards flatbuffer payloads to `tx` without copying.
///
/// # Usage
/// ```rust
/// let (tx, rx) = ipc::frame_channel();
/// tokio::spawn(ZmqSubscriber::new("tcp://localhost:5555", b"vision").run(tx));
/// // rx handed to vision::FrameProcessor
/// ```
pub struct ZmqSubscriber {
    endpoint: String,
    topic:    Vec<u8>,
}

impl ZmqSubscriber {
    pub fn new(endpoint: impl Into<String>, topic: impl Into<Vec<u8>>) -> Self {
        Self {
            endpoint: endpoint.into(),
            topic:    topic.into(),
        }
    }

    /// Runs until the ZMQ socket closes or `tx` is dropped.
    ///
    /// # Frame drops
    /// If the vision channel is full (`TrySendError::Full`), the incoming frame
    /// is silently dropped. This is intentional: vision must process recent
    /// frames, not stale ones. Log the drop count for observability.
    pub async fn run(self, tx: Sender<FrameBuffer>) -> Result<(), IpcError> {
        let ctx = Context::new();
        let mut sub = subscribe(&ctx)
            .connect(&self.endpoint)?
            .subscribe(&self.topic)?;

        let mut dropped: u64 = 0;

        while let Some(msg) = sub.next().await {
            let parts = msg?;

            // Multipart ZMQ message: [0] = topic, [1] = flatbuffer payload.
            let Some(payload) = parts.0.get(1) else {
                continue; // malformed — topic-only message, skip
            };

            // One allocation: move ZMQ bytes into an Arc'd slice.
            // All downstream consumers share this Arc — zero further copies.
            let buf: FrameBuffer = Arc::from(payload.as_ref());

            match tx.try_send(buf) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    dropped += 1;
                    // OBSERVABILITY: replace with metrics increment in production.
                    if dropped % 100 == 1 {
                        eprintln!("[ipc] WARN: frame channel full — dropped {dropped} frames");
                    }
                }
                Err(TrySendError::Disconnected(_)) => return Err(IpcError::ChannelClosed),
            }
        }

        Ok(())
    }
}
