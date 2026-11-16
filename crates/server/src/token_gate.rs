//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! The bearer token every call on the remote listener must present, as a layer
//! in front of every service there. The local socket and pipe have no such
//! layer: the OS already says who may connect to them.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tonic::body::Body;
use tower::{Layer, Service};

/// The header a token travels in, as `Bearer <token>`.
const AUTHORIZATION: &str = "authorization";

/// Adds [`TokenGate`] around a service. Cheap to clone: the token is shared.
#[derive(Clone)]
pub struct TokenGateLayer {
    token: Arc<[u8]>,
}

impl std::fmt::Debug for TokenGateLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenGateLayer").finish_non_exhaustive()
    }
}

impl TokenGateLayer {
    /// A gate admitting calls that present exactly `token`.
    #[must_use]
    pub fn new(token: Vec<u8>) -> Self {
        Self {
            token: token.into(),
        }
    }
}

impl<S> Layer<S> for TokenGateLayer {
    type Service = TokenGate<S>;

    fn layer(&self, inner: S) -> TokenGate<S> {
        TokenGate {
            inner,
            token: Arc::clone(&self.token),
        }
    }
}

/// Fails a call without `authorization: Bearer <token>` naming this daemon's
/// token with `UNAUTHENTICATED`, before any handler or other layer runs, and
/// without saying whether the header was missing or wrong.
///
/// Concurrency: `Clone`, and as concurrent as the service it wraps.
/// Performance: one header lookup and a comparison over the token's length.
#[derive(Clone)]
pub struct TokenGate<S> {
    inner: S,
    token: Arc<[u8]>,
}

/// Whether `a` equals `b`, in time that depends on their lengths only, so the
/// time a refusal takes does not tell a caller how much of a guess was right.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether `request` presents `token`.
fn admitted<B>(request: &http::Request<B>, token: &[u8]) -> bool {
    let Some(value) = request.headers().get(AUTHORIZATION) else {
        return false;
    };
    let Some(presented) = value.as_bytes().strip_prefix(b"Bearer ") else {
        return false;
    };
    same(presented, token)
}

impl<S, B> Service<http::Request<B>> for TokenGate<S>
where
    S: Service<http::Request<B>, Response = http::Response<Body>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        if admitted(&request, &self.token) {
            return Box::pin(self.inner.call(request));
        }
        let status = tonic::Status::unauthenticated("a valid bearer token is required");
        Box::pin(std::future::ready(Ok(status.into_http())))
    }
}

#[cfg(test)]
mod tests {
    use super::{admitted, same};

    fn request(header: Option<&str>) -> http::Request<()> {
        let mut builder = http::Request::builder();
        if let Some(value) = header {
            builder = builder.header("authorization", value);
        }
        builder.body(()).unwrap_or_default()
    }

    #[test]
    fn only_the_exact_token_after_bearer_is_admitted() {
        let token = b"0123456789abcdef0123456789abcdef";
        let right = "Bearer 0123456789abcdef0123456789abcdef";
        assert!(admitted(&request(Some(right)), token));
        assert!(!admitted(&request(None), token));
        assert!(!admitted(&request(Some("Bearer 0123456789abcdef")), token));
        assert!(!admitted(
            &request(Some(&right.replace("Bearer", "Basic"))),
            token
        ));
        assert!(!admitted(&request(Some(&format!("{right}0"))), token));
        assert!(!admitted(
            &request(Some("Bearer 0123456789abcdef0123456789abcdeF")),
            token
        ));
    }

    #[test]
    fn comparison_needs_equal_length_and_bytes() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
        assert!(same(b"", b""));
    }
}
