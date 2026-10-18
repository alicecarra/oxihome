//! Login to the web interface: one password, sessions kept in SQLite.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Context;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use jiff::{SignedDuration, Timestamp};
use password_hash::{SaltString, rand_core::OsRng};
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Body, Layer, LayerFuture, Next, Path,
        content::Form,
        error::{SeeOther, redirect, see_other, unauthorized},
        header::{HeaderName, HeaderValue},
        page, parse_query_params,
        request::{headers, method, uri},
        response::response_headers,
        route,
    },
    session,
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::format::query_value;
use super::icons;
use super::install::{self, install_links};
use crate::components::button::button;
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::components::label::label;
use crate::storage::Sessions;

pub const LIFETIME: Duration = Duration::from_hours(24 * 30);
/// A session in use is extended once it is this much older than a fresh one.
const REFRESH_AFTER: SignedDuration = SignedDuration::from_hours(24);
/// Wrong passwords allowed within `FAILURE_WINDOW`.
const MAX_FAILURES: usize = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
const FAILURE_DELAY: Duration = Duration::from_secs(1);
/// What a signed-out browser can still load.
const PUBLIC_PATHS: [&str; 2] = ["/login", "/static/app.css"];

pub struct Auth {
    /// Argon2 hash in PHC format; the interface is open when missing.
    password_hash: Option<String>,
    sessions: Sessions,
    failures: Failures,
}

impl Auth {
    pub fn new(password_hash: Option<String>, sessions: Sessions) -> anyhow::Result<Self> {
        if let Some(hash) = &password_hash {
            PasswordHash::new(hash)
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("password_hash is not an argon2 hash from `oxihome-hub hash-password`")?;
        }
        Ok(Self {
            password_hash,
            sessions,
            failures: Failures::default(),
        })
    }

    pub fn enabled(&self) -> bool {
        self.password_hash.is_some()
    }

    async fn verify(&self, password: String) -> bool {
        let Some(hash) = self.password_hash.clone() else {
            return false;
        };
        // Argon2 takes tens of milliseconds of CPU on purpose.
        tokio::task::spawn_blocking(move || {
            PasswordHash::new(&hash).is_ok_and(|hash| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &hash)
                    .is_ok()
            })
        })
        .await
        .unwrap_or(false)
    }
}

/// Recent wrong passwords, counted for everyone together.
#[derive(Default)]
struct Failures(Mutex<VecDeque<Instant>>);

impl Failures {
    fn blocked(&self, now: Instant) -> bool {
        let mut failures = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        while failures
            .front()
            .is_some_and(|failure| now.duration_since(*failure) >= FAILURE_WINDOW)
        {
            failures.pop_front();
        }
        failures.len() >= MAX_FAILURES
    }

    fn record(&self, now: Instant) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(now);
    }
}

/// The PHC string for `[http] password_hash` in the secrets file.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .to_string())
}

/// Whether the request may use the interface: always when there is no password, otherwise with
/// a stored, unexpired session, which is extended while in use.
async fn signed_in(cx: &Cx) -> Result<bool> {
    let auth: &Auth = app_context(cx);
    if !auth.enabled() {
        return Ok(true);
    }
    let Some(token_hash) = session::token_hash(cx).await? else {
        return Ok(false);
    };
    let expires_at = match auth.sessions.expiry(&token_hash).await {
        Ok(Some(expires_at)) => expires_at,
        Ok(None) => return Ok(false),
        Err(error) => {
            println!("web: reading a session: {error:#}");
            return Ok(false);
        }
    };
    let fresh = Timestamp::now() + LIFETIME;
    if fresh.duration_since(expires_at) > REFRESH_AFTER
        && let Some(refreshed) = session::refresh(cx).await?
    {
        save(auth, &refreshed).await;
    }
    Ok(true)
}

async fn save(auth: &Auth, session: &session::Session) -> bool {
    let result = match Timestamp::try_from(session.expires_at) {
        Ok(expires_at) => auth.sessions.save(&session.token_hash, expires_at).await,
        Err(error) => Err(error.into()),
    };
    if let Err(error) = &result {
        println!("web: saving a session: {error:#}");
    }
    result.is_ok()
}

/// Sends signed-out requests to `/login`: pages by redirect, htmx requests by `HX-Redirect`
/// (so the login replaces the whole page, not a fragment), anything else gets 401.
pub struct AuthLayer;

impl Layer for AuthLayer {
    fn path(&self) -> Option<&Path> {
        Some(Path::new("/"))
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(async move {
            let path = uri(cx).path();
            if PUBLIC_PATHS.contains(&path)
                || install::PUBLIC_PATHS.contains(&path)
                || signed_in(cx).await?
            {
                return next.run(cx, body).await;
            }
            let request_headers = headers(cx);
            if request_headers.contains_key("hx-request") {
                let current = request_headers
                    .get("hx-current-url")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|url| url.parse::<topcoat::router::Uri>().ok())
                    .and_then(|url| url.path_and_query().map(ToString::to_string));
                let location = login_location(current.as_deref().unwrap_or("/"));
                if let Ok(value) = HeaderValue::from_str(&location) {
                    response_headers(cx).append(HeaderName::from_static("hx-redirect"), value);
                }
                return Err(unauthorized().into());
            }
            if method(cx) == topcoat::router::Method::GET && uri(cx).path() != "/events" {
                let requested = uri(cx).path_and_query().map_or("/", |path| path.as_str());
                return Err(redirect(login_location(requested)).into());
            }
            Err(unauthorized().into())
        })
    }
}

fn login_location(next: &str) -> String {
    if next == "/" || !is_local(next) {
        "/login".to_string()
    } else {
        format!("/login?next={}", query_value(next))
    }
}

/// A path on this hub, not `//other.host` or `/\other.host`, which browsers read as other sites.
/// Browsers also drop tabs and line breaks from URLs, so `/<TAB>/other.host` would become one.
fn is_local(target: &str) -> bool {
    target.starts_with('/')
        && !target.starts_with("//")
        && !target.contains('\\')
        && !target
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

/// Over plain HTTP, except on localhost, the browser drops the session cookie (it is `Secure`),
/// so the login would seem to do nothing.
fn plain_http(cx: &Cx) -> bool {
    let request_headers = headers(cx);
    let header = |name| {
        request_headers
            .get(name)
            .and_then(|value: &HeaderValue| value.to_str().ok())
    };
    let host = header("host").unwrap_or_default();
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    let local = ["localhost", "127.0.0.1", "[::1]"].contains(&name);
    !local && header("x-forwarded-proto") != Some("https")
}

fn next_target(next: Option<&str>) -> &str {
    next.filter(|next| is_local(next)).unwrap_or("/")
}

#[page("/login")]
pub async fn login_page(cx: &Cx) -> Result<impl View> {
    let query = parse_query_params::<HashMap<String, String>>(cx).unwrap_or_default();
    let next = next_target(query.get("next").map(String::as_str)).to_string();
    if signed_in(cx).await? {
        return Err(redirect(next).into());
    }
    let plain_http = plain_http(cx);
    Ok(view! { login_document(next: next, error: None, plain_http: plain_http) })
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
    next: Option<String>,
}

#[route(POST "/login")]
pub async fn login(cx: &Cx, Form(form): Form<LoginForm>) -> Result<ViewHandle> {
    let auth: &Auth = app_context(cx);
    let next = next_target(form.next.as_deref()).to_string();
    if !auth.enabled() {
        return Err(see_other(next).into());
    }
    let error = if auth.failures.blocked(Instant::now()) {
        "Too many wrong passwords. Try again in a minute."
    } else if auth.verify(form.password).await {
        let session = session::start(cx).await?;
        if save(auth, &session).await {
            return Err(see_other(next).into());
        }
        "Could not save the session; see the hub's log."
    } else {
        auth.failures.record(Instant::now());
        tokio::time::sleep(FAILURE_DELAY).await;
        "Wrong password."
    };
    let plain_http = plain_http(cx);
    view! { cx => login_document(next: next, error: Some(error), plain_http: plain_http) }
        .single()
        .await
}

#[route(POST "/logout")]
pub async fn logout(cx: &Cx) -> Result<SeeOther> {
    let auth: &Auth = app_context(cx);
    if let Some(token_hash) = session::stop(cx).await?
        && let Err(error) = auth.sessions.delete(&token_hash).await
    {
        println!("web: deleting a session: {error:#}");
    }
    Ok(see_other("/login"))
}

/// A page of its own instead of the layout, which would load the menu and live updates.
#[component]
async fn login_document(
    next: String,
    error: Option<&'static str>,
    plain_http: bool,
) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html class="dark">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Oxihome"</title>
                <link rel="icon" type="image/svg+xml" href=(icons::FAVICON)>
                <link rel="stylesheet" href="/static/app.css">
                install_links()
            </head>
            <body>
                <main class="flex min-h-screen items-center justify-center p-4">
                    card(
                        attrs: attributes! { class="w-full max-w-sm" },
                        card_header(
                            card_title("Oxihome")
                            card_description("Enter the password to continue.")
                        )
                        card_content(
                            <form method="post" action="/login" class="flex flex-col gap-4">
                                <input type="hidden" name="next" value=(next)>
                                // Lets password managers file the password under a name.
                                <input type="hidden" name="username" value="oxihome" autocomplete="username">
                                <div class="flex flex-col gap-2">
                                    label(attrs: attributes! { for="password" }, "Password")
                                    input(attrs: attributes! {
                                        id="password"
                                        name="password"
                                        type="password"
                                        autocomplete="current-password"
                                        required=""
                                        autofocus=""
                                        aria-invalid=(if error.is_some() { "true" } else { "false" })
                                    })
                                    if let Some(error) = error {
                                        <p class="text-sm text-destructive">(error)</p>
                                    }
                                </div>
                                button(attrs: attributes! { type="submit" class="w-full" }, "Sign in")
                                if plain_http {
                                    <p class="text-sm text-muted-foreground">
                                        "This address is plain HTTP, so the browser will not keep the login. Open the hub over HTTPS, or on localhost."
                                    </p>
                                }
                            </form>
                        )
                    )
                </main>
            </body>
        </html>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_next_on_this_hub() {
        assert_eq!(next_target(Some("/nodes/a?x=1")), "/nodes/a?x=1");
        for target in [
            "//evil.example",
            "/\\evil.example",
            "/\t/evil.example",
            "/\n/evil.example",
            "/ /evil.example",
            "https://evil.example",
            "nodes",
        ] {
            assert_eq!(next_target(Some(target)), "/", "{target}");
        }
        assert_eq!(next_target(None), "/");
        assert_eq!(login_location("/"), "/login");
        assert_eq!(login_location("/nodes?a=b"), "/login?next=%2Fnodes%3Fa%3Db");
    }

    #[test]
    fn blocks_after_too_many_failures() {
        let failures = Failures::default();
        let start = Instant::now();
        for _ in 0..MAX_FAILURES {
            assert!(!failures.blocked(start));
            failures.record(start);
        }
        assert!(failures.blocked(start));
        assert!(!failures.blocked(start + FAILURE_WINDOW));
    }

    #[test]
    fn verifies_its_own_hash() {
        let hash = hash_password("correct horse").unwrap();
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(
            Argon2::default()
                .verify_password(b"correct horse", &parsed)
                .is_ok()
        );
        assert!(
            Argon2::default()
                .verify_password(b"wrong", &parsed)
                .is_err()
        );
    }
}
