use anyhow::{Context, bail};
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::{Client, StatusCode};

const MAX_SIZE: usize = 5 * 1024 * 1024;
const UNTITLED: &str = "(untitled)";

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
    etag: Option<String>,
    last_modified: Option<String>,
}

impl Fetcher {
    pub fn new(url: String) -> Self {
        Self {
            url,
            etag: None,
            last_modified: None,
        }
    }

    pub async fn fetch(&mut self, client: &Client) -> anyhow::Result<Fetched> {
        let mut request = client.get(&self.url);
        if let Some(etag) = &self.etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = &self.last_modified {
            request = request.header(IF_MODIFIED_SINCE, last_modified);
        }
        let response = request.send().await?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        let response = response.error_for_status()?;
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
        let body = response.bytes().await?;
        if body.len() > MAX_SIZE {
            bail!("larger than {MAX_SIZE} bytes");
        }
        let items = parse(&body)?;
        self.etag = etag;
        self.last_modified = last_modified;
        Ok(Fetched::Items(items))
    }
}

/// RSS, Atom or JSON Feed; feeds list the newest first, items without a date keep that order.
pub fn parse(body: &[u8]) -> anyhow::Result<Vec<Item>> {
    let feed = feed_rs::parser::parse(body).context("reading the feed")?;
    let mut items: Vec<Item> = feed.entries.into_iter().rev().map(item).collect();
    items.sort_by_key(|item| item.date);
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
}
