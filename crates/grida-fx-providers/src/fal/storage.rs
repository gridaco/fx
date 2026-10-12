//! A bounded fal storage upload before a paid motion submission. The public request remains
//! digest-backed; the returned URLs live only in this call. The engine owns every retry.

use crate::adapter::Submitted;
use crate::setup::Client;
use crate::transport::{
    Body, HttpRequest, HttpResponse, Lane, Method, TransportError, TransportErrorKind,
};
use grida_fx_core::money::Usd;
use serde_json::json;
use std::time::Duration;

const INITIATE: &str = "https://rest.fal.ai/storage/upload/initiate?storage_type=fal-cdn-v3";
const LABEL: &str = "fal motion reference upload";
const DEADLINE: Duration = Duration::from_secs(300);
const RESPONSE_BYTES: u64 = 1024 * 1024;

/// One storage initiation and one unauthenticated PUT, then the transient model input URL.
/// No paid submit has been made, even if a free upload fails after sending its bytes.
pub(super) async fn motion(client: &Client, bytes: Vec<u8>) -> Result<String, Submitted> {
    let credential = client
        .credential(super::CREDENTIAL_HEADER, super::CREDENTIAL_PREFIX)
        .map_err(|reason| refused(client, &reason))?;
    let initiate = HttpRequest::new(Method::Post, INITIATE, Lane::Provider)
        .credential(credential)
        .header(
            "x-fal-object-lifecycle",
            "{\"expiration_duration_seconds\":3600}",
        )
        .body(Body::Json(
            json!({"content_type":"video/mp4","file_name":"motion.mp4"}),
        ))
        .timeout(DEADLINE)
        .max_response_bytes(RESPONSE_BYTES);
    let response = client
        .send(initiate)
        .await
        .map_err(|error| transport(client, &error))?;
    status(client, &response)?;
    let payload = crate::wire::json_object(&response.body, LABEL).map_err(|_| {
        refused(
            client,
            "fal upload returned no usable storage slot; video submission was not sent",
        )
    })?;
    let field = |name: &str| {
        payload
            .get(name)
            .and_then(serde_json::Value::as_str)
            .filter(|url| super::is_fal_media_url(url))
    };
    let (Some(upload_url), Some(file_url)) = (field("upload_url"), field("file_url")) else {
        return Err(refused(
            client,
            "fal upload returned a URL outside fal media; video submission was not sent",
        ));
    };
    let mut upload_client = client.clone();
    upload_client.redactor = client.redactor.with(upload_url).with(file_url);
    let upload = HttpRequest::new(Method::Put, upload_url, Lane::Upload)
        .body(Body::Bytes {
            content_type: "video/mp4".into(),
            bytes,
        })
        .timeout(DEADLINE)
        .max_response_bytes(RESPONSE_BYTES);
    let response = upload_client
        .send(upload)
        .await
        .map_err(|error| transport(&upload_client, &error))?;
    status(&upload_client, &response)?;
    Ok(file_url.to_string())
}

fn refused(client: &Client, reason: &str) -> Submitted {
    Submitted::Refused {
        reason: client.reason(reason),
    }
}

fn transport(client: &Client, error: &TransportError) -> Submitted {
    let reason = format!(
        "{LABEL} failed; video submission was not sent: {}",
        error.reason
    );
    if error.kind == TransportErrorKind::Refused {
        refused(client, &reason)
    } else {
        super::submit_not_received(client, &reason, None)
    }
}

#[allow(clippy::result_large_err)]
fn status(client: &Client, response: &HttpResponse) -> Result<(), Submitted> {
    if response.is_success() {
        return Ok(());
    }
    let reason = format!(
        "{}; video submission was not sent",
        super::status_text(LABEL, response)
    );
    Err(match response.status {
        408 | 429 | 500..=599 => {
            super::submit_not_received(client, &reason, super::retry_after(response))
        }
        _ => Submitted::Failed {
            reason: client.reason(&reason),
            cost: Some(Usd::ZERO),
            retryable: false,
        },
    })
}
