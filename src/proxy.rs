use crate::upstream;
use async_trait::async_trait;
use bytes::Bytes;
use pingora::proxy::{ProxyHttp, Session};
use pingora::upstreams::peer::HttpPeer;
use pingora::Result;
use std::net::SocketAddr;
use std::time::Instant;
use tracing::info;

/// RFC 3339 UTC timestamp for the nginx-style access log line.
fn rfc3339_now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "-".to_string())
}

pub struct Proxy {
    pub upstream: upstream::Upstream,
    pub replacement: String,
    /// Built once in `Proxy::new` from the startup-resolved address: no per-request
    /// DNS, no per-request resolve cost. Cloned per request — HttpPeer: Clone copies
    /// the same reuse hash / group_key, so cloned peers still share the pooled
    /// upstream connections (pool identity is peer data, not object identity).
    pub peer: HttpPeer,
}

pub struct Ctx {
    buf: Vec<u8>,           // full request body, matched requests only
    start: Option<Instant>, // request start, for the access-log duration
}

impl Proxy {
    pub fn new(upstream: upstream::Upstream, replacement: String) -> Self {
        let socket_addr = SocketAddr::new(upstream.socket_addr, upstream.port);
        let peer = HttpPeer::new(socket_addr, upstream.tls, upstream.sni.clone());
        Proxy {
            upstream,
            replacement,
            peer,
        }
    }

    fn is_matched(&self, session: &Session) -> bool {
        // uri.path() never contains the query string (approved: query ignored for
        // matching, forwarded unmodified).
        session.req_header().uri.path() == self.upstream.pattern
    }
}

#[async_trait]
impl ProxyHttp for Proxy {
    type CTX = Ctx;

    fn new_ctx(&self) -> Self::CTX {
        Ctx {
            buf: Vec::new(),
            start: Some(Instant::now()),
        }
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        // Host replacement: every request goes to the upstream origin at the
        // received path, verbatim. TLS from the URL scheme; SNI = host.
        // The Box is forced by the trait signature (owned peer per request);
        // everything under it is a cheap clone of the pre-built peer.
        Ok(Box::new(self.peer.clone()))
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut pingora::http::RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Zed sends Host: localhost:4000 — the provider must see its own host.
        // insert_header has replace semantics (no duplicate Host).
        upstream_request.insert_header("Host", &self.upstream.sni)?;
        if self.is_matched(session) {
            // Verified (finalize_h1_upstream_request_framing): a body-bearing HTTP/1
            // upstream request left without Content-Length and without
            // Transfer-Encoding after the filters gets `Transfer-Encoding: chunked`
            // inserted. Keeping the stale downstream Content-Length with a
            // differently-sized rewritten body would be a framing violation — drop it.
            upstream_request.remove_header("Content-Length");
        }
        Ok(())
    }

    async fn request_body_filter(
        &self,
        session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        if !self.is_matched(session) {
            return Ok(()); // pure pass-through: no inspection, chunks forwarded untouched
        }

        // Verified mechanics (send_body_to_pipe):
        // - `None` after the filter sends HttpTask::Body(None, true) — that TERMINATES
        //   the upstream body. So `body.take()` alone is wrong for mid-body chunks.
        // - A `Some(empty)` chunk on a NON-final chunk is skipped by the 0-byte guard
        //   ("request_body_filter decides not to output anything yet") without ending
        //   the body — that is the suppression mechanism.
        // - The chunk the filter assigns on the final call becomes the final body
        //   write, followed by finish_body().
        if let Some(chunk) = body.take() {
            ctx.buf.extend_from_slice(&chunk);
        }

        if end_of_stream {
            // Emit exactly one final chunk: rewritten body, or the original buffer
            // when nothing was rewritten (absent field, other value, parse error).
            let out = match crate::rewrite::rewrite(&ctx.buf, &self.replacement) {
                Some(new_body) => new_body,
                None => std::mem::take(&mut ctx.buf),
            };
            // Clear the buffer: on an upstream retry the retry buffer replays the
            // FULL original body through this filter again as one chunk — a non-empty
            // ctx.buf here would duplicate it.
            ctx.buf.clear();
            if out.is_empty() {
                *body = None; // bodyless final: Body(None, true) → finish_body only
            } else {
                *body = Some(Bytes::from(out));
            }
        } else {
            // Suppress forwarding of the buffered-but-not-yet-complete chunk.
            *body = Some(Bytes::new()); // skipped by the 0-byte guard
        }
        Ok(())
    }

    /// Access-log line per request, nginx combined-style (TASK.md §13).
    /// Called when the response is fully sent or the request terminally failed —
    /// the same timing nginx writes its access-log entry.
    async fn logging(&self, session: &mut Session, e: Option<&pingora::Error>, ctx: &mut Self::CTX) {
        let req = session.req_header();
        let client = session
            .downstream_session
            .client_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "-".to_string());
        let status = session
            .response_written()
            .map_or(0, |resp| resp.status.as_u16());
        let bytes_sent = session.downstream_session.body_bytes_sent();
        let dur_ms = ctx
            .start
            .as_ref()
            .map(|t| t.elapsed().as_millis())
            .unwrap_or_default();
        let user_agent = req
            .headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-");
        let method = &req.method;
        let path = req.uri.path();
        let version = match req.version {
            pingora::http::Version::HTTP_09 => "HTTP/0.9",
            pingora::http::Version::HTTP_10 => "HTTP/1.0",
            pingora::http::Version::HTTP_11 => "HTTP/1.1",
            pingora::http::Version::HTTP_2 => "HTTP/2.0",
            pingora::http::Version::HTTP_3 => "HTTP/3.0",
            _ => "HTTP/?",
        };
        let matched = path == self.upstream.pattern;
        // verbatim client addr, method path version, status, bytes, timestamp,
        // UA, duration, rewrite marker, error
        info!(
            target: "zedproxy::access",
            client = %client,
            method = %method,
            path = %path,
            version = version,
            status,
            bytes_sent,
            matched,
            duration_ms = dur_ms,
            error = ?e,
            "[{}] \"{} {} {}\" {} {} +{}ms rewrite={} err={:?} \"{}\"",
            rfc3339_now(),
            method,
            path,
            version,
            status,
            bytes_sent,
            dur_ms,
            matched,
            e,
            user_agent
        );
    }
}
