use std::sync::LazyLock;

use anyhow::{Context, bail};
use regex::Regex;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::{Client, StatusCode, Url};

const MAX_SIZE: usize = 5 * 1024 * 1024;
const UNTITLED: &str = "(untitled)";
/// Not plain `application/json`, which WordPress uses for its REST API.
const FEED_TYPES: [&str; 3] = [
    "application/rss+xml",
    "application/atom+xml",
    "application/feed+json",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub link: String,
    /// Seconds since the epoch: when it was published, or else updated.
    pub date: Option<i64>,
}

pub enum Fetched {
    /// Oldest first.
    Items(Vec<Item>),
    NotModified,
}

/// One feed's URL and what its server said last time, for conditional requests.
pub struct Fetcher {
    url: String,
    /// The feed linked from the page at `url`, when `url` is a page: found once per run.
    discovered: Option<Url>,
    etag: Option<String>,
    last_modified: Option<String>,
}

struct Download {
    /// After redirects.
    url: Url,
    body: Vec<u8>,
    etag: Option<String>,
    last_modified: Option<String>,
}

impl Fetcher {
    pub fn new(url: String) -> Self {
        Self {
            url,
            discovered: None,
            etag: None,
            last_modified: None,
        }
    }

    /// A page instead of a feed (a YouTube channel, a blog's home) is followed to the feed it links.
    pub async fn fetch(&mut self, client: &Client) -> anyhow::Result<Fetched> {
        let url = match &self.discovered {
            Some(feed) => feed.to_string(),
            None => self.url.clone(),
        };
        let Some(mut download) = self.download(client, &url).await? else {
            return Ok(Fetched::NotModified);
        };
        let items = match parse(&download.body) {
            Ok(items) => items,
            Err(error) if self.discovered.is_none() => {
                let Some(feed) = discover(&download.body, &download.url) else {
                    return Err(error.context("no feed linked from the page either"));
                };
                println!("{}: following the feed it links, {feed}", self.url);
                self.discovered = Some(feed.clone());
                self.etag = None;
                self.last_modified = None;
                download = self
                    .download(client, feed.as_str())
                    .await?
                    .context("not modified on the first request")?;
                parse(&download.body)?
            }
            Err(error) => return Err(error),
        };
        self.etag = download.etag;
        self.last_modified = download.last_modified;
        Ok(Fetched::Items(items))
    }

    /// `None` when not modified.
    async fn download(&self, client: &Client, url: &str) -> anyhow::Result<Option<Download>> {
        let mut request = client.get(url);
        if let Some(etag) = &self.etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = &self.last_modified {
            request = request.header(IF_MODIFIED_SINCE, last_modified);
        }
        let response = request.send().await?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        let mut response = response.error_for_status()?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_SIZE as u64)
        {
            bail!("larger than {MAX_SIZE} bytes");
        }
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let etag = header(ETAG);
        let last_modified = header(LAST_MODIFIED);
        let url = response.url().clone();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > MAX_SIZE {
                bail!("larger than {MAX_SIZE} bytes");
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Some(Download {
            url,
            body,
            etag,
            last_modified,
        }))
    }
}

/// The page's first `<link rel="alternate">` to an RSS, Atom or JSON feed, the one browsers find.
pub fn discover(page: &[u8], base: &Url) -> Option<Url> {
    static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<link\b[^>]*>").unwrap());
    static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)([a-z-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#).unwrap()
    });
    let page = String::from_utf8_lossy(page);
    LINK.find_iter(&page).find_map(|tag| {
        let mut rel = None;
        let mut kind = None;
        let mut href = None;
        for attribute in ATTRIBUTE.captures_iter(tag.as_str()) {
            let Some(value) = (2..=4).find_map(|group| attribute.get(group)) else {
                continue;
            };
            let value = value.as_str();
            match attribute[1].to_ascii_lowercase().as_str() {
                "rel" => rel = Some(value),
                "type" => kind = Some(value),
                "href" => href = Some(value),
                _ => {}
            }
        }
        let alternate = rel?
            .split_whitespace()
            .any(|rel| rel.eq_ignore_ascii_case("alternate"));
        let feed = FEED_TYPES
            .iter()
            .any(|feed_type| kind.is_some_and(|kind| kind.trim().eq_ignore_ascii_case(feed_type)));
        if !alternate || !feed {
            return None;
        }
        base.join(html_escape::decode_html_entities(href?).trim())
            .ok()
    })
}

/// RSS, Atom or JSON Feed; feeds list the newest first, items without a date keep that order.
pub fn parse(body: &[u8]) -> anyhow::Result<Vec<Item>> {
    let feed = feed_rs::parser::parse(body).context("reading the feed")?;
    let mut items: Vec<Item> = feed.entries.into_iter().rev().map(item).collect();
    let dated: Vec<usize> = (0..items.len())
        .filter(|&index| items[index].date.is_some())
        .collect();
    let mut sorted: Vec<Item> = dated.iter().map(|&index| items[index].clone()).collect();
    sorted.sort_by_key(|item| item.date);
    for (index, item) in dated.into_iter().zip(sorted) {
        items[index] = item;
    }
    Ok(items)
}

fn item(entry: feed_rs::model::Entry) -> Item {
    let link = entry
        .links
        .iter()
        .find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate"))
        .or(entry.links.first())
        .map(|link| link.href.clone())
        .unwrap_or_default();
    let title = entry
        .title
        .map(|title| {
            // Some feeds escape their titles twice, and JSON Feed titles often carry HTML entities.
            html_escape::decode_html_entities(&title.content)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| UNTITLED.into());
    Item {
        id: entry.id,
        title,
        link,
        date: entry
            .published
            .or(entry.updated)
            .map(|date| date.timestamp()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atom_items_come_oldest_first() {
        let atom = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>tag:github.com,2008:releases</id>
  <title>Release notes</title>
  <updated>2026-10-14T12:00:00Z</updated>
  <entry>
    <id>tag:github.com,2008:Repository/1/v0.2.0</id>
    <updated>2026-10-14T12:00:00Z</updated>
    <link rel="alternate" type="text/html" href="https://github.com/example/project/releases/tag/v0.2.0"/>
    <title>v0.2.0
      the second</title>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1/v0.1.0</id>
    <updated>2026-10-01T12:00:00Z</updated>
    <link rel="alternate" type="text/html" href="https://github.com/example/project/releases/tag/v0.1.0"/>
    <title>v0.1.0</title>
  </entry>
</feed>"#;
        let items = parse(atom).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "v0.1.0");
        assert_eq!(items[1].title, "v0.2.0 the second");
        assert_eq!(
            items[1].link,
            "https://github.com/example/project/releases/tag/v0.2.0"
        );
        assert_eq!(items[1].id, "tag:github.com,2008:Repository/1/v0.2.0");
    }

    #[test]
    fn rss_items_without_dates_keep_the_feed_order_reversed() {
        let rss = br#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Blog</title>
  <link>https://example.com</link>
  <description>Posts</description>
  <item><title>Newest</title><link>https://example.com/3</link><guid>3</guid></item>
  <item><link>https://example.com/2</link><guid>2</guid></item>
  <item><title>Oldest</title><link>https://example.com/1</link><guid>1</guid></item>
</channel></rss>"#;
        let items = parse(rss).unwrap();
        let titles: Vec<_> = items.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(titles, ["Oldest", UNTITLED, "Newest"]);
        assert_eq!(items[2].id, "3");
    }

    #[test]
    fn items_without_dates_stay_in_place_among_dated_ones() {
        let rss = br#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Blog</title>
  <link>https://example.com</link>
  <description>Posts</description>
  <item><title>C</title><guid>c</guid><pubDate>Wed, 14 Oct 2026 12:00:00 GMT</pubDate></item>
  <item><title>Undated</title><guid>u</guid></item>
  <item><title>A</title><guid>a</guid><pubDate>Mon, 12 Oct 2026 12:00:00 GMT</pubDate></item>
  <item><title>B</title><guid>b</guid><pubDate>Tue, 13 Oct 2026 12:00:00 GMT</pubDate></item>
</channel></rss>"#;
        let items = parse(rss).unwrap();
        let titles: Vec<_> = items.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(titles, ["A", "B", "Undated", "C"]);
    }

    #[test]
    fn json_feed_titles_lose_their_html_entities() {
        let json = br#"{
            "version": "https://jsonfeed.org/version/1.1",
            "title": "Notebook",
            "items": [{
                "id": "https://example.com/kings/",
                "url": "https://example.com/kings/",
                "title": "There&#39;s no King &amp; no &quot;Kings&quot;",
                "date_published": "2026-10-01T12:00:00Z"
            }]
        }"#;
        let items = parse(json).unwrap();
        assert_eq!(items[0].title, "There's no King & no \"Kings\"");
        assert_eq!(items[0].link, "https://example.com/kings/");
    }

    #[test]
    fn not_a_feed() {
        assert!(parse(b"<html><body>Not found</body></html>").is_err());
    }

    #[test]
    fn a_youtube_channel_page_links_its_feed() {
        let page = br#"<!DOCTYPE html><html><head>
<link rel="alternate" media="handheld" href="https://m.youtube.com/@GoogleDevelopers">
<link rel="alternate" type="application/rss+xml" title="RSS" href="https://www.youtube.com/feeds/videos.xml?channel_id=UC_x5XG1OV2P6uZZ5FSM9Ttw">
<link rel="canonical" href="https://www.youtube.com/channel/UC_x5XG1OV2P6uZZ5FSM9Ttw">
</head><body></body></html>"#;
        let base = Url::parse("https://www.youtube.com/@GoogleDevelopers").unwrap();
        assert_eq!(
            discover(page, &base).unwrap().as_str(),
            "https://www.youtube.com/feeds/videos.xml?channel_id=UC_x5XG1OV2P6uZZ5FSM9Ttw"
        );
    }

    #[test]
    fn a_relative_feed_link_with_entities() {
        let page = br#"<html><head>
<LINK type='application/json' rel='alternate' href='/wp-json/wp/v2/pages/2'>
<link href=/feed/?lang=pt&amp;full=1 type="application/atom+xml" rel="alternate home">
</head></html>"#;
        let base = Url::parse("https://blog.example.com/posts/").unwrap();
        assert_eq!(
            discover(page, &base).unwrap().as_str(),
            "https://blog.example.com/feed/?lang=pt&full=1"
        );
    }

    #[test]
    fn a_page_without_a_feed_link() {
        let page = br#"<html><head><link rel="stylesheet" type="text/css" href="/style.css"></head></html>"#;
        let base = Url::parse("https://example.com/").unwrap();
        assert_eq!(discover(page, &base), None);
    }
}
