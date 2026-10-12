use topcoat::{
    context::Cx,
    router::{Body, Layer, LayerFuture, Next, Path, error::forbidden, header, request::headers},
};

/// Rejects requests whose `Host` is not the hub's own name. Against DNS rebinding: a site that
/// points its domain at 127.0.0.1 would otherwise count as same-origin and get past the router's
/// origin check.
pub struct HostLayer {
    allowed: Vec<String>,
}

impl HostLayer {
    /// `localhost` and the loopback addresses, plus the address the hub listens on.
    pub fn new(listen_host: &str) -> Self {
        let mut allowed: Vec<String> = ["localhost", "127.0.0.1", "[::1]"]
            .into_iter()
            .map(String::from)
            .collect();
        allowed.push(listen_host.to_ascii_lowercase());
        Self { allowed }
    }

    fn allows(&self, host: &str) -> bool {
        let name = match host.rsplit_once(':') {
            Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
            _ => host,
        };
        self.allowed
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name))
    }
}

impl Layer for HostLayer {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        let host = headers(cx)
            .get(header::HOST)
            .and_then(|value| value.to_str().ok());
        if host.is_some_and(|host| self.allows(host)) {
            next.run(cx, body)
        } else {
            Box::pin(async { Err(forbidden().into()) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_only_the_hub_names() {
        let layer = HostLayer::new("192.168.15.10");
        for host in [
            "localhost:8080",
            "127.0.0.1:8080",
            "[::1]:8080",
            "LOCALHOST",
            "192.168.15.10:8080",
        ] {
            assert!(layer.allows(host), "{host}");
        }
        for host in [
            "evil.example:8080",
            "evil.example",
            "localhost.evil.example",
            "[::1]x",
        ] {
            assert!(!layer.allows(host), "{host}");
        }
    }
}
