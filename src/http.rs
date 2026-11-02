use anyhow::Result;
use bytes::Bytes;
use futures_util::StreamExt;
use crate::progress::{Event, Progress};

/// Emulates the Chrome TLS/HTTP2 fingerprint so Cloudflare doesn't block us
pub fn client() -> Result<wreq::Client> {
    Ok(wreq::Client::builder()
        .emulation(wreq_util::emulate::Emulation::Chrome136)
        .build()?)
}

/// Reads a response body, reporting progress as it arrives
pub async fn read_body(response: wreq::Response, progress: &Progress) -> Result<Bytes> {
    let total = response.content_length();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk?);
        progress.emit(Event::Download { done: body.len() as u64, total });
    }
    Ok(body.into())
}
