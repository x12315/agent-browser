use std::time::Duration;

use tokio_tungstenite::connect_async;
use url::Url;

use super::firefox::DevtoolsEndpoint;

/// A Firefox remote agent to attach to: the BiDi WebSocket and the DevTools (RDP) endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachTarget {
    pub bidi_url: String,
    pub devtools: DevtoolsEndpoint,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn target_from_url(mut url: Url) -> Result<AttachTarget, String> {
    let bidi_scheme = if matches!(url.scheme(), "https" | "wss") {
        "wss"
    } else {
        "ws"
    };
    let host = match url
        .host()
        .ok_or_else(|| "Firefox endpoint must include a host".to_string())?
    {
        url::Host::Ipv6(host) => host.to_string(),
        host => host.to_string(),
    };
    let devtools_port = url
        .query_pairs()
        .find(|(key, _)| key == "devtools")
        .map(|(_, value)| {
            value
                .parse::<u16>()
                .map_err(|_| "Firefox endpoint has an invalid devtools port".to_string())
        })
        .transpose()?
        .unwrap_or(6000);
    let query: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "devtools")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_scheme(bidi_scheme)
        .map_err(|_| "Firefox endpoint has an invalid WebSocket scheme".to_string())?;
    url.set_path("/session");
    if query.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(query);
    }
    Ok(AttachTarget {
        bidi_url: url.to_string(),
        devtools: DevtoolsEndpoint::Tcp {
            host,
            port: devtools_port,
        },
    })
}

/// Parses a `ws://` or `wss://` URL whose path is `/session` into a target without network access.
pub fn parse_attach_url(target: &str) -> Result<Option<AttachTarget>, String> {
    let Ok(url) = Url::parse(target) else {
        return Ok(None);
    };
    if !matches!(url.scheme(), "ws" | "wss") || url.path() != "/session" {
        return Ok(None);
    }
    target_from_url(url).map(Some)
}

/// Probes a port or HTTP URL for Firefox's WebDriver BiDi WebSocket.
pub async fn probe_attach_target(target: &str) -> Result<AttachTarget, String> {
    let url = if let Ok(port) = target.parse::<u16>() {
        Url::parse(&format!("http://127.0.0.1:{port}")).unwrap()
    } else {
        Url::parse(target).map_err(|e| format!("invalid Firefox endpoint: {e}"))?
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Err("expected an http(s)://host:port URL or a port".into());
    }
    let bidi = target_from_url(url)?;
    let connect = tokio::time::timeout(PROBE_TIMEOUT, connect_async(&bidi.bidi_url)).await;
    match connect {
        Ok(Ok((mut socket, _))) => {
            let _ = socket.close(None).await;
            Ok(bidi)
        }
        Ok(Err(error)) => Err(format!("WebSocket handshake failed: {error}")),
        Err(_) => Err("WebSocket handshake timed out after 2 seconds".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_attach_url, target_from_url, AttachTarget};
    use crate::native::bidi::firefox::DevtoolsEndpoint;
    use url::Url;

    fn target(url: &str, expected_url: &str, expected_port: u16) {
        assert_eq!(
            parse_attach_url(url).unwrap(),
            Some(AttachTarget {
                bidi_url: expected_url.into(),
                devtools: DevtoolsEndpoint::Tcp {
                    host: "127.0.0.1".into(),
                    port: expected_port,
                },
            })
        );
    }

    #[test]
    fn parses_webdriver_bidi_attach_urls() {
        target(
            "ws://127.0.0.1:9333/session",
            "ws://127.0.0.1:9333/session",
            6000,
        );
        target(
            "ws://127.0.0.1:9333/session?devtools=6080",
            "ws://127.0.0.1:9333/session",
            6080,
        );
        target(
            "wss://127.0.0.1:9333/session?token=a&devtools=6080",
            "wss://127.0.0.1:9333/session?token=a",
            6080,
        );
        assert_eq!(parse_attach_url("ws://127.0.0.1:9333/other"), Ok(None));
        assert!(
            parse_attach_url("ws://127.0.0.1:9333/session?devtools=nope")
                .unwrap_err()
                .contains("invalid devtools port")
        );
        assert_eq!(
            parse_attach_url("ws://[::1]:9333/session").unwrap(),
            Some(AttachTarget {
                bidi_url: "ws://[::1]:9333/session".into(),
                devtools: DevtoolsEndpoint::Tcp {
                    host: "::1".into(),
                    port: 6000,
                },
            })
        );
    }

    #[test]
    fn http_targets_keep_their_transport_security_when_probed() {
        let secure = target_from_url(Url::parse("https://127.0.0.1?token=a").unwrap()).unwrap();
        assert_eq!(secure.bidi_url, "wss://127.0.0.1/session?token=a");
        let plain = target_from_url(Url::parse("http://127.0.0.1").unwrap()).unwrap();
        assert_eq!(plain.bidi_url, "ws://127.0.0.1/session");
    }
}
