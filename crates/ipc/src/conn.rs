//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! A connection to the daemon, with what each call on it presents: nothing on a
//! local endpoint, whose OS checks who connects, and a bearer token on a daemon's
//! `[remote]` listener.

use std::task::{Context, Poll};

use tonic::body::Body;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tower::Service;

/// The `authorization` header each call carries, if any.
///
/// Concurrency: cloned with the connection; the header is shared.
#[derive(Clone, Default)]
pub struct Credentials(Option<MetadataValue<Ascii>>);

impl Credentials {
    /// `authorization: Bearer <token>` on every call. `None` when `token` cannot
    /// travel in a header: it must be printable ASCII without spaces.
    #[must_use]
    pub fn bearer(token: &str) -> Option<Self> {
        let usable = !token.is_empty() && token.bytes().all(|b| b.is_ascii_graphic());
        let value = format!("Bearer {token}").parse().ok().filter(|_| usable)?;
        Some(Self(Some(value)))
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown = if self.0.is_some() { "bearer" } else { "none" };
        f.debug_tuple("Credentials").field(&shown).finish()
    }
}

impl Interceptor for Credentials {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(value) = &self.0 {
            request
                .metadata_mut()
                .insert("authorization", value.clone());
        }
        Ok(request)
    }
}

/// A channel to the daemon and the credentials each call on it presents. Any
/// generated client takes it, as it takes a [`Channel`], and a [`Channel`]
/// converts into one presenting nothing, so a function taking `impl Into<Conn>`
/// accepts either.
///
/// Concurrency: `Clone`, sharing the channel's connection, as a [`Channel`] does.
#[derive(Clone, Debug)]
pub struct Conn(InterceptedService<Channel, Credentials>);

impl Conn {
    /// `channel`, with `credentials` on every call.
    #[must_use]
    pub fn new(channel: Channel, credentials: Credentials) -> Self {
        Self(InterceptedService::new(channel, credentials))
    }
}

impl From<Channel> for Conn {
    fn from(channel: Channel) -> Self {
        Self::new(channel, Credentials::default())
    }
}

type Inner = InterceptedService<Channel, Credentials>;

impl Service<http::Request<Body>> for Conn {
    type Response = <Inner as Service<http::Request<Body>>>::Response;
    type Error = <Inner as Service<http::Request<Body>>>::Error;
    type Future = <Inner as Service<http::Request<Body>>>::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        self.0.call(request)
    }
}

#[cfg(test)]
mod tests {
    use super::Credentials;

    #[test]
    fn a_token_that_cannot_be_a_header_value_is_refused() {
        assert!(Credentials::bearer("0123456789abcdef").is_some());
        assert!(Credentials::bearer("").is_none());
        assert!(Credentials::bearer("has space").is_none());
        assert!(Credentials::bearer("line\nbreak").is_none());
    }
}
