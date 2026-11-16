//! SPDX-FileCopyrightText: 2026 Srikanth K
//! SPDX-License-Identifier: AGPL-3.0-or-later

//! `nornctl quota`: read a stream's hard quota, set it, remove it.

use std::io::Write;

use norn_ipc::Conn;
use norn_protocol::stream::stream_service_client::StreamServiceClient;
use norn_protocol::stream::{GetStreamRequest, StorageQuota, Stream, UpdateStreamRequest};

use crate::CliError;
use crate::resources::{call, usage};
use crate::upkeep::stream_name;

const USAGE: &str = "quota get STREAM | set STREAM Nb|none";

/// A quota from `5000000b`, or none from `none`.
fn quota(word: &str) -> Result<Option<StorageQuota>, CliError> {
    if word == "none" {
        return Ok(None);
    }
    let bytes = word.strip_suffix('b').ok_or_else(|| usage(USAGE))?;
    let max_bytes = bytes.parse().map_err(|_| usage(USAGE))?;
    Ok(Some(StorageQuota { max_bytes }))
}

fn show(quota: Option<&StorageQuota>) -> String {
    quota.map_or_else(
        || "none".to_owned(),
        |q| format!("max_bytes={}", q.max_bytes),
    )
}

/// Runs a `quota` subcommand.
pub(crate) async fn run(
    channel: Conn,
    words: &[&str],
    out: &mut impl Write,
) -> Result<(), CliError> {
    let mut client = StreamServiceClient::new(channel);
    match words {
        ["get", stream] => {
            let name = stream_name(stream);
            let got = client
                .get_stream(GetStreamRequest { name: name.clone() })
                .await;
            let got = got.map_err(call("StreamService.GetStream"))?.into_inner();
            Ok(writeln!(out, "{name} {}", show(got.quota.as_ref()))?)
        }
        ["set", stream, word] => {
            let name = stream_name(stream);
            let request = UpdateStreamRequest {
                stream: Some(Stream {
                    name: name.clone(),
                    quota: quota(word)?,
                    ..Stream::default()
                }),
                update_mask: Some(prost_types::FieldMask {
                    paths: vec!["quota".to_owned()],
                }),
                validate_only: false,
            };
            let set = client.update_stream(request).await;
            let set = set
                .map_err(call("StreamService.UpdateStream"))?
                .into_inner();
            Ok(writeln!(out, "{name} {}", show(set.quota.as_ref()))?)
        }
        _ => Err(usage(USAGE)),
    }
}
